//! The rules and routes over rtnetlink: listing, adding back, deleting, and
//! the socket the guard hears deletions on. Messages are
//! netlink-packet-route's (already in sail's graph, through tun-rs); the
//! socket is netlink-sys's.

use std::io;
use std::net::IpAddr;
use std::os::fd::{AsRawFd, RawFd};

use netlink_packet_core::{
    NetlinkHeader, NetlinkMessage, NetlinkPayload, NLM_F_ACK, NLM_F_CREATE, NLM_F_DUMP,
    NLM_F_DUMP_INTR, NLM_F_EXCL, NLM_F_REQUEST,
};
use netlink_packet_route::route::{
    RouteAddress, RouteAttribute, RouteHeader, RouteMessage, RouteProtocol, RouteScope, RouteType,
};
use netlink_packet_route::rule::{
    RuleAction, RuleAttribute, RuleFlags, RuleMessage, RulePortRange, RuleUidRange,
};
use netlink_packet_route::{AddressFamily, IpProtocol, RouteNetlinkMessage};
use netlink_sys::{protocols::NETLINK_ROUTE, Socket, SocketAddr};

use super::{Action, Family, Prefix, Route, Rule};

/// How many times a dump the kernel interrupted (the rules changed while it
/// was read) is started over.
const DUMP_ATTEMPTS: usize = 5;

/// A request socket: one request at a time, each waited for.
pub(super) struct Netlink {
    socket: Socket,
    seq: u32,
}

impl Netlink {
    pub(super) fn open() -> io::Result<Netlink> {
        let mut socket = Socket::new(NETLINK_ROUTE)?;
        socket.bind_auto()?;
        socket.connect(&SocketAddr::new(0, 0))?;
        Ok(Netlink { socket, seq: 0 })
    }

    /// Every rule of both families.
    pub(super) fn rules(&mut self) -> io::Result<Vec<Rule>> {
        let request = RouteNetlinkMessage::GetRule(RuleMessage::default());
        let mut rules = Vec::new();
        for message in self.dump(request)? {
            if let RouteNetlinkMessage::NewRule(m) = message {
                rules.extend(parse_rule(&m));
            }
        }
        Ok(rules)
    }

    /// The routes of `table` through link `oif`, both families.
    pub(super) fn routes(&mut self, table: u32, oif: u32) -> io::Result<Vec<Route>> {
        Ok(self
            .table_routes(table)?
            .iter()
            .filter_map(parse_route)
            .filter(|r| r.oif == oif)
            .collect())
    }

    /// Every route of `table`, as listed (to delete them).
    fn table_routes(&mut self, table: u32) -> io::Result<Vec<RouteMessage>> {
        let request = RouteNetlinkMessage::GetRoute(RouteMessage::default());
        let mut routes = Vec::new();
        for message in self.dump(request)? {
            if let RouteNetlinkMessage::NewRoute(m) = message {
                if route_table(&m) == table {
                    routes.push(m);
                }
            }
        }
        Ok(routes)
    }

    pub(super) fn add_rule(&mut self, rule: &Rule) -> io::Result<()> {
        let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
        self.ack(RouteNetlinkMessage::NewRule(rule_message(rule)), flags)
    }

    pub(super) fn add_route(&mut self, route: &Route, table: u32) -> io::Result<()> {
        let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
        self.ack(
            RouteNetlinkMessage::NewRoute(route_message(route, table)),
            flags,
        )
    }

    /// Deletes every rule of either family at a priority in `priorities`,
    /// and every route of `table`, whatever made them; returns how many.
    pub(super) fn delete_scope(
        &mut self,
        priorities: std::ops::RangeInclusive<u32>,
        table: u32,
    ) -> io::Result<usize> {
        let mut deleted = 0;
        for family in [AddressFamily::Inet, AddressFamily::Inet6] {
            for priority in priorities.clone() {
                // A delete with only a priority takes the first rule there.
                loop {
                    let mut m = RuleMessage::default();
                    m.header.family = family;
                    m.attributes.push(RuleAttribute::Priority(priority));
                    match self.ack(RouteNetlinkMessage::DelRule(m), NLM_F_REQUEST | NLM_F_ACK) {
                        Ok(()) => deleted += 1,
                        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => break,
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        for route in self.table_routes(table)? {
            match self.ack(
                RouteNetlinkMessage::DelRoute(route),
                NLM_F_REQUEST | NLM_F_ACK,
            ) {
                Ok(()) => deleted += 1,
                // Gone with another (a device's routes go with it).
                Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(deleted)
    }

    fn send(&mut self, message: RouteNetlinkMessage, flags: u16) -> io::Result<u32> {
        self.seq = self.seq.wrapping_add(1);
        let mut header = NetlinkHeader::default();
        header.flags = flags;
        header.sequence_number = self.seq;
        let mut packet = NetlinkMessage::new(header, NetlinkPayload::from(message));
        packet.finalize();
        let mut buf = vec![0; packet.buffer_len()];
        packet.serialize(&mut buf);
        self.socket.send(&buf, 0)?;
        Ok(self.seq)
    }

    /// Sends a request whose answer is only an acknowledgement.
    fn ack(&mut self, message: RouteNetlinkMessage, flags: u16) -> io::Result<()> {
        let seq = self.send(message, flags)?;
        loop {
            let (buf, _) = self.socket.recv_from_full()?;
            for m in messages(&buf)? {
                if m.header.sequence_number != seq {
                    continue;
                }
                if let NetlinkPayload::Error(e) = m.payload {
                    return match e.code {
                        None => Ok(()),
                        Some(_) => Err(e.to_io()),
                    };
                }
            }
        }
    }

    /// Sends a dump request and reads every answer to it; a dump the kernel
    /// interrupted is started over.
    fn dump(&mut self, message: RouteNetlinkMessage) -> io::Result<Vec<RouteNetlinkMessage>> {
        'attempt: for _ in 0..DUMP_ATTEMPTS {
            let seq = self.send(message.clone(), NLM_F_REQUEST | NLM_F_DUMP)?;
            let mut out = Vec::new();
            let mut interrupted = false;
            loop {
                let (buf, _) = self.socket.recv_from_full()?;
                for m in messages(&buf)? {
                    if m.header.sequence_number != seq {
                        continue;
                    }
                    interrupted |= m.header.flags & NLM_F_DUMP_INTR != 0;
                    match m.payload {
                        NetlinkPayload::Done(_) if interrupted => continue 'attempt,
                        NetlinkPayload::Done(_) => return Ok(out),
                        NetlinkPayload::Error(e) if e.code.is_some() => return Err(e.to_io()),
                        NetlinkPayload::InnerMessage(inner) => out.push(inner),
                        _ => {}
                    }
                }
            }
        }
        Err(io::Error::other("the kernel kept interrupting the dump"))
    }
}

/// The socket deletions are heard on: rules and routes of both families.
pub(super) struct Watch {
    socket: Socket,
}

impl Watch {
    pub(super) fn open() -> io::Result<Watch> {
        let mut socket = Socket::new(NETLINK_ROUTE)?;
        socket.bind_auto()?;
        for group in [
            libc::RTNLGRP_IPV4_RULE,
            libc::RTNLGRP_IPV6_RULE,
            libc::RTNLGRP_IPV4_ROUTE,
            libc::RTNLGRP_IPV6_ROUTE,
        ] {
            socket.add_membership(group)?;
        }
        socket.set_non_blocking(true)?;
        Ok(Watch { socket })
    }

    /// The notifications waiting, without blocking: Ok(empty) when there
    /// are none. ENOBUFS (some were lost) is an error the caller answers
    /// with a check.
    pub(super) fn read(&self) -> io::Result<Vec<Notification>> {
        let mut out = Vec::new();
        loop {
            let buf = match self.socket.recv_from_full() {
                Ok((buf, _)) => buf,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(out),
                Err(e) => return Err(e),
            };
            if buf.is_empty() {
                return Ok(out);
            }
            for m in messages(&buf)? {
                let sender = m.header.port_number;
                match m.payload {
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelRule(rule)) => {
                        if let Some(rule) = parse_rule(&rule) {
                            out.push(Notification::RuleDeleted {
                                priority: rule.priority,
                                sender,
                            });
                        }
                    }
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelRoute(route)) => {
                        out.push(Notification::RouteDeleted {
                            table: route_table(&route),
                            sender,
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

impl AsRawFd for Watch {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

/// A deletion, and the netlink port of who asked for it (0: the kernel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Notification {
    RuleDeleted { priority: u32, sender: u32 },
    RouteDeleted { table: u32, sender: u32 },
}

fn messages(buf: &[u8]) -> io::Result<Vec<NetlinkMessage<RouteNetlinkMessage>>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < buf.len() {
        let m = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&buf[offset..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = m.header.length as usize;
        out.push(m);
        if len == 0 {
            break;
        }
        // Messages are 4-byte aligned.
        offset += (len + 3) & !3;
    }
    Ok(out)
}

fn family(family: AddressFamily) -> Option<Family> {
    match family {
        AddressFamily::Inet => Some(Family::V4),
        AddressFamily::Inet6 => Some(Family::V6),
        _ => None,
    }
}

fn address_family(family: Family) -> AddressFamily {
    match family {
        Family::V4 => AddressFamily::Inet,
        Family::V6 => AddressFamily::Inet6,
    }
}

/// A listed rule; None for a family other than IPv4 and IPv6.
fn parse_rule(m: &RuleMessage) -> Option<Rule> {
    let mut r = Rule::new(family(m.header.family)?, 0, Action::Nop);
    r.invert = m.header.flags.contains(RuleFlags::Invert);
    r.tos = m.header.tos;
    let mut table = u32::from(m.header.table);
    let mut goto = 0;
    let (mut mark, mut mask) = (None, None);
    for a in &m.attributes {
        match a {
            RuleAttribute::Priority(p) => r.priority = *p,
            RuleAttribute::Table(t) => table = *t,
            RuleAttribute::Goto(to) => goto = *to,
            RuleAttribute::Source(addr) => r.src = Some(Prefix::new(*addr, m.header.src_len)),
            RuleAttribute::Destination(addr) => r.dst = Some(Prefix::new(*addr, m.header.dst_len)),
            RuleAttribute::Iifname(name) => r.iif = Some(name.clone()),
            RuleAttribute::Oifname(name) => r.oif = Some(name.clone()),
            RuleAttribute::FwMark(v) => mark = Some(*v),
            RuleAttribute::FwMask(v) => mask = Some(*v),
            // u32::MAX is "none", as `ip rule` shows it.
            RuleAttribute::SuppressPrefixLen(n) if *n != u32::MAX => {
                r.suppress_prefixlen = Some(*n)
            }
            RuleAttribute::IpProtocol(p) => r.ip_proto = Some(u8::from(*p)),
            RuleAttribute::DestinationPortRange(p) => r.dport = Some((p.start, p.end)),
            RuleAttribute::SourcePortRange(p) => r.sport = Some((p.start, p.end)),
            RuleAttribute::UidRange(u) => r.uid_range = Some((u.start, u.end)),
            _ => {}
        }
    }
    if mark.is_some() || mask.is_some() {
        r.fwmark = Some((mark.unwrap_or(0), mask.unwrap_or(u32::MAX)));
    }
    r.action = match m.header.action {
        RuleAction::ToTable => Action::Table(table),
        RuleAction::Goto => Action::Goto(goto),
        RuleAction::Nop => Action::Nop,
        RuleAction::Blackhole => Action::Blackhole,
        RuleAction::Unreachable => Action::Unreachable,
        RuleAction::Prohibit => Action::Prohibit,
        other => Action::Other(u8::from(other)),
    };
    if !matches!(r.action, Action::Table(_)) {
        r.suppress_prefixlen = None;
    }
    Some(r)
}

/// The message that adds `r` back, as `ip rule add` words it.
fn rule_message(r: &Rule) -> RuleMessage {
    let mut m = RuleMessage::default();
    m.header.family = address_family(r.family);
    m.header.tos = r.tos;
    if r.invert {
        m.header.flags |= RuleFlags::Invert;
    }
    let a = &mut m.attributes;
    a.push(RuleAttribute::Priority(r.priority));
    if let Some(src) = r.src {
        m.header.src_len = src.len;
        a.push(RuleAttribute::Source(src.addr));
    }
    if let Some(dst) = r.dst {
        m.header.dst_len = dst.len;
        a.push(RuleAttribute::Destination(dst.addr));
    }
    if let Some(iif) = &r.iif {
        a.push(RuleAttribute::Iifname(iif.clone()));
    }
    if let Some(oif) = &r.oif {
        a.push(RuleAttribute::Oifname(oif.clone()));
    }
    if let Some((mark, mask)) = r.fwmark {
        a.push(RuleAttribute::FwMark(mark));
        a.push(RuleAttribute::FwMask(mask));
    }
    if let Some(proto) = r.ip_proto {
        a.push(RuleAttribute::IpProtocol(IpProtocol::from(proto)));
    }
    if let Some((start, end)) = r.sport {
        a.push(RuleAttribute::SourcePortRange(RulePortRange { start, end }));
    }
    if let Some((start, end)) = r.dport {
        a.push(RuleAttribute::DestinationPortRange(RulePortRange {
            start,
            end,
        }));
    }
    if let Some((start, end)) = r.uid_range {
        a.push(RuleAttribute::UidRange(RuleUidRange { start, end }));
    }
    m.header.action = match r.action {
        Action::Table(table) => {
            // The header's table is a byte; the attribute carries it all.
            m.header.table = u8::try_from(table).unwrap_or(0);
            a.push(RuleAttribute::Table(table));
            if let Some(len) = r.suppress_prefixlen {
                a.push(RuleAttribute::SuppressPrefixLen(len));
            }
            RuleAction::ToTable
        }
        Action::Goto(to) => {
            a.push(RuleAttribute::Goto(to));
            RuleAction::Goto
        }
        Action::Nop => RuleAction::Nop,
        Action::Blackhole => RuleAction::Blackhole,
        Action::Unreachable => RuleAction::Unreachable,
        Action::Prohibit => RuleAction::Prohibit,
        Action::Other(n) => RuleAction::from(n),
    };
    m
}

/// The table of a listed route: the attribute, else the header's byte.
fn route_table(m: &RouteMessage) -> u32 {
    m.attributes
        .iter()
        .find_map(|a| match a {
            RouteAttribute::Table(t) => Some(*t),
            _ => None,
        })
        .unwrap_or(u32::from(m.header.table))
}

fn route_address(a: &RouteAddress) -> Option<IpAddr> {
    match a {
        RouteAddress::Inet(v4) => Some(IpAddr::V4(*v4)),
        RouteAddress::Inet6(v6) => Some(IpAddr::V6(*v6)),
        _ => None,
    }
}

/// A listed unicast route through one interface; None for anything else.
fn parse_route(m: &RouteMessage) -> Option<Route> {
    let family = family(m.header.address_family)?;
    if m.header.kind != RouteType::Unicast {
        return None;
    }
    // A default route has no destination attribute.
    let unspecified = match family {
        Family::V4 => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        Family::V6 => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
    };
    let mut route = Route {
        dst: Prefix::new(unspecified, m.header.destination_prefix_length),
        gateway: None,
        oif: 0,
        metric: 0,
    };
    for a in &m.attributes {
        match a {
            RouteAttribute::Destination(d) => route.dst.addr = route_address(d)?,
            RouteAttribute::Gateway(g) => route.gateway = route_address(g),
            RouteAttribute::Oif(index) => route.oif = *index,
            RouteAttribute::Priority(p) => route.metric = *p,
            _ => {}
        }
    }
    (route.oif != 0).then_some(route)
}

/// The message that adds `r` back to `table`, as `ip route add` words it.
fn route_message(r: &Route, table: u32) -> RouteMessage {
    let mut m = RouteMessage::default();
    m.header.address_family = address_family(super::family_of(r.dst.addr));
    m.header.destination_prefix_length = r.dst.len;
    m.header.table = u8::try_from(table).unwrap_or(0);
    m.header.protocol = RouteProtocol::Boot;
    m.header.scope = if r.gateway.is_some() {
        RouteScope::Universe
    } else {
        RouteScope::Link
    };
    m.header.kind = RouteType::Unicast;
    let address = |addr: IpAddr| match addr {
        IpAddr::V4(v4) => RouteAddress::Inet(v4),
        IpAddr::V6(v6) => RouteAddress::Inet6(v6),
    };
    let a = &mut m.attributes;
    a.push(RouteAttribute::Table(table));
    if r.dst.len > 0 {
        a.push(RouteAttribute::Destination(address(r.dst.addr)));
    }
    if let Some(gw) = r.gateway {
        a.push(RouteAttribute::Gateway(address(gw)));
    }
    a.push(RouteAttribute::Oif(r.oif));
    a.push(RouteAttribute::Priority(r.metric));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every selector and action survives a trip through the messages.
    #[test]
    fn rules_read_back_as_written() {
        let rules = [
            Rule {
                dst: Some(Prefix::new("10.60.159.88".parse().unwrap(), 30)),
                ..Rule::new(Family::V4, 9091, Action::Table(2091))
            },
            Rule {
                suppress_prefixlen: Some(0),
                ..Rule::new(Family::V6, 9092, Action::Table(2091))
            },
            Rule {
                invert: true,
                dport: Some((53, 53)),
                suppress_prefixlen: Some(0),
                ..Rule::new(Family::V4, 9093, Action::Table(super::super::TABLE_MAIN))
            },
            Rule {
                iif: Some("lo".into()),
                src: Some(Prefix::new("8000::".parse().unwrap(), 1)),
                ..Rule::new(Family::V6, 9093, Action::Goto(9101))
            },
            Rule {
                fwmark: Some((0x2024, u32::MAX)),
                oif: Some("eth0".into()),
                ip_proto: Some(17),
                sport: Some((1000, 2000)),
                uid_range: Some((1000, 1000)),
                tos: 0x10,
                ..Rule::new(Family::V4, 9095, Action::Unreachable)
            },
            Rule::new(Family::V6, 9101, Action::Nop),
        ];
        for rule in rules {
            let back = parse_rule(&rule_message(&rule)).unwrap();
            assert_eq!(back, rule, "{rule}");
        }
    }

    #[test]
    fn routes_read_back_as_written() {
        let routes = [
            Route {
                dst: Prefix::new("0.0.0.0".parse().unwrap(), 0),
                gateway: None,
                oif: 7,
                metric: 0,
            },
            Route {
                dst: Prefix::new("fc00::".parse().unwrap(), 7),
                gateway: Some("fde2:ec40:9312:c7fd::2".parse().unwrap()),
                oif: 7,
                metric: 1024,
            },
        ];
        for route in routes {
            let m = route_message(&route, 2091);
            assert_eq!(route_table(&m), 2091);
            assert_eq!(parse_route(&m).unwrap(), route, "{route}");
        }
    }
}
