//! Detects other proxy / VPN clients that compete for the network path
//! (ported from the unused Tauri `detect.rs`). Offline and cheap; it only
//! reports, never blocks.
//!
//! Signals:
//! 1. Running processes matching a keyword table (sysinfo).
//! 2. A tunnel interface holding the fake-IP range 198.18.0.0/15 (Surge,
//!    Clash & co. hijack DNS with it).
//! 3. The route to the internet going through another app's tunnel: the
//!    interface the routing table picks for a public address, or another
//!    app's tunnel holding a default / split-default route (`0/1` +
//!    `128.0/1`, what Surge's and Clash's enhanced modes install; macOS
//!    `netstat -rn`, Linux `/proc/net/route`, Windows `GetIpForwardTable2`). The
//!    table catches it even while our own TUN wins `route get`.
//! 4. The system DNS resolver in the fake-IP range (e.g. Surge's
//!    198.18.0.2), only while our own TUN is down.
//!
//! A process alone is not a conflict (an open app is not an active tunnel):
//! competitors are reported only when signal 2, 3 or 4 holds. Tunnel-only apps
//! (Tailscale, ZeroTier) count only when they hold the route. Our own TUN
//! (10.60.159.89, 172.19.0.1 before core 0.5.7; see ppvpn-core's builder)
//! and our own processes are excluded.

use std::net::Ipv4Addr;

/// Addresses of our own TUN interface (ppvpn-core `addTUN`): 0.5.7 moved
/// it off 172.19.0.1, the sing-box default other tunnels use too; a core
/// from before still runs until the service is reinstalled.
const OWN_TUN_ADDRESSES: [Ipv4Addr; 2] =
    [Ipv4Addr::new(10, 60, 159, 89), Ipv4Addr::new(172, 19, 0, 1)];

/// Result of [`crate::Client::detect_conflicts`].
#[derive(uniffi::Record, Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictReport {
    /// Apps actively competing (running *and* a tunnel signal holds), by
    /// display name. Empty means no conflict.
    pub competitors: Vec<String>,
    /// Known proxy/VPN apps that are running, conflicting or not.
    pub observed: Vec<String>,
    /// Other apps' tunnel interfaces holding fake-IP addresses, e.g.
    /// `utun5 (198.18.0.1)`.
    pub fake_ip_interfaces: Vec<String>,
    /// The interface of another app's tunnel that currently carries traffic
    /// to the internet, or holds a default / split-default route.
    pub foreign_default_route: Option<String>,
    /// System DNS resolvers in the fake-IP range (not ours), e.g.
    /// `198.18.0.2`.
    pub fake_ip_dns: Vec<String>,
}

/// One running process.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub exe: String,
}

/// One network interface with its IPv4 addresses. `name` is what the OS
/// shows (BSD name, Linux ifname, Windows alias + description).
#[derive(Clone, Debug, Default)]
pub(crate) struct InterfaceInfo {
    pub name: String,
    pub ipv4: Vec<Ipv4Addr>,
}

/// Everything the detector looks at; gathered per OS, injected in tests.
#[derive(Clone, Debug, Default)]
pub(crate) struct Inputs {
    pub processes: Vec<ProcessInfo>,
    pub interfaces: Vec<InterfaceInfo>,
    /// Interface carrying traffic to the internet.
    pub route_interface: Option<String>,
    /// Interfaces holding an IPv4 default or split-default route, in table
    /// order (interface-scoped routes excluded).
    pub default_routes: Vec<String>,
    /// System DNS resolvers (IPv4).
    pub resolvers: Vec<Ipv4Addr>,
    /// Our own process id.
    pub own_pid: u32,
}

#[derive(Clone, Copy)]
enum Match {
    /// Substring of the process name or executable path.
    Contains(&'static str),
    /// Whole process name (short names that would over-match).
    Exact(&'static str),
}

struct Known {
    matcher: Match,
    label: &'static str,
    /// Mesh/corporate tunnels that normally leave the default route alone:
    /// only a conflict when they hold the route.
    tunnel_only: bool,
}

const fn known(matcher: Match, label: &'static str) -> Known {
    Known {
        matcher,
        label,
        tunnel_only: false,
    }
}

const fn tunnel(matcher: Match, label: &'static str) -> Known {
    Known {
        matcher,
        label,
        tunnel_only: true,
    }
}

use Match::{Contains, Exact};

/// Apps on every desktop OS.
const COMMON: &[Known] = &[
    known(Contains("clash verge"), "Clash Verge"),
    known(Contains("clash-verge"), "Clash Verge"),
    known(Contains("verge-mihomo"), "Clash Verge"),
    known(Contains("mihomo party"), "Mihomo Party"),
    known(Contains("mihomo-party"), "Mihomo Party"),
    known(Contains("mihomo"), "Mihomo"),
    known(Contains("clash-meta"), "Clash Meta"),
    known(Contains("clash-nyanpasu"), "Clash Nyanpasu"),
    known(Contains("flclash"), "FlClash"),
    known(Exact("clash"), "Clash"),
    known(Exact("xray"), "Xray"),
    known(Exact("v2ray"), "V2Ray"),
    known(Contains("nekobox"), "NekoBox"),
    known(Contains("nekoray"), "NekoRay"),
    known(Contains("hiddify"), "Hiddify"),
    known(Contains("sing-box"), "sing-box"),
    known(Contains("gui.for.singbox"), "GUI.for.SingBox"),
    known(Contains("wireguard"), "WireGuard"),
    known(Contains("openvpn"), "OpenVPN"),
    known(Contains("nordvpn"), "NordVPN"),
    known(Contains("mullvad"), "Mullvad"),
    known(Contains("expressvpn"), "ExpressVPN"),
    known(Contains("globalprotect"), "GlobalProtect"),
    known(Contains("zscaler"), "Zscaler"),
    tunnel(Contains("tailscale"), "Tailscale"),
    tunnel(Contains("zerotier"), "ZeroTier"),
];

#[cfg(target_os = "macos")]
const PLATFORM: &[Known] = &[
    known(Contains("surge"), "Surge"),
    known(Contains("clashx"), "ClashX"),
    known(Contains("v2rayu"), "V2RayU"),
    known(Contains("tunnelblick"), "Tunnelblick"),
    known(Contains("shadowrocket"), "Shadowrocket"),
    known(Exact("stash"), "Stash"),
    known(Contains("stash.app"), "Stash"),
    known(Contains("quantumult x"), "Quantumult X"),
    known(Exact("loon"), "Loon"),
    known(Contains("loon.app"), "Loon"),
    known(Exact("sfm"), "sing-box"),
    known(Contains("egern"), "Egern"),
    known(Contains("cisco secure client"), "Cisco AnyConnect"),
    known(Contains("vpnagentd"), "Cisco AnyConnect"),
    known(Contains("anyconnect"), "Cisco AnyConnect"),
    known(Exact("pangpa"), "GlobalProtect"),
    known(Exact("pangps"), "GlobalProtect"),
];

#[cfg(windows)]
const PLATFORM: &[Known] = &[
    known(Contains("clash for windows"), "Clash for Windows"),
    known(Contains("v2rayn"), "v2rayN"),
    known(Contains("clash-win"), "Clash"),
    known(Contains("vpnui"), "Cisco AnyConnect"),
    known(Contains("vpnagent"), "Cisco AnyConnect"),
    known(Contains("csc_ui"), "Cisco AnyConnect"),
    known(Contains("pangpa"), "GlobalProtect"),
    known(Contains("pangps"), "GlobalProtect"),
    known(Contains("zsatunnel"), "Zscaler"),
    known(Contains("zsatray"), "Zscaler"),
];

#[cfg(target_os = "linux")]
const PLATFORM: &[Known] = &[
    known(Contains("vpnagentd"), "Cisco AnyConnect"),
    known(Contains("anyconnect"), "Cisco AnyConnect"),
    known(Exact("gpclient"), "GlobalProtect"),
    known(Exact("gpservice"), "GlobalProtect"),
    known(Contains("zsatray"), "Zscaler"),
    known(Exact("wg-quick"), "WireGuard"),
    known(Contains("clash-linux"), "Clash"),
    known(Contains("v2raya"), "v2rayA"),
];

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
const PLATFORM: &[Known] = &[];

fn table() -> impl Iterator<Item = &'static Known> {
    PLATFORM.iter().chain(COMMON)
}

fn is_own_process(process: &ProcessInfo, own_pid: u32) -> bool {
    let name = process.name.to_lowercase();
    let exe = process.exe.to_lowercase();
    process.pid == own_pid
        || name.starts_with("ppvpn")
        || exe.contains("/ppvpn.app/")
        || exe.contains("\\ppvpn\\")
        || exe.ends_with("/ppvpn")
}

/// The known app a process belongs to.
fn classify(process: &ProcessInfo) -> Option<&'static Known> {
    let name = process.name.to_lowercase();
    let file = process
        .exe
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let exe = process.exe.to_lowercase();
    // The process's own name first, the install path only then: a
    // `clash-meta` binary under `/opt/mihomo/` is Clash Meta, not Mihomo.
    table()
        .find(|known| match known.matcher {
            Contains(keyword) => name.contains(keyword) || file.contains(keyword),
            Exact(keyword) => {
                name == keyword || file == keyword || file.trim_end_matches(".exe") == keyword
            }
        })
        .or_else(|| {
            table().find(|known| match known.matcher {
                Contains(keyword) => exe.contains(keyword),
                Exact(_) => false,
            })
        })
}

/// Apps built on the Clash / mihomo core, which share TUN names (mihomo's
/// default `Meta`, `clash0`): a tunnel named after the core is shown as the
/// running app of the family that owns it (Mihomo, Clash Meta, Clash
/// Verge, ...), not as the generic core name.
const CLASH_FAMILY: &[&str] = &[
    "Mihomo",
    "Clash Meta",
    "Clash",
    "Clash Verge",
    "Mihomo Party",
    "Clash Nyanpasu",
    "FlClash",
    "ClashX",
    "Clash for Windows",
];

/// `owner` (named after a tunnel interface), refined to the running app of
/// the same family when there is one.
fn refine_owner<'a>(owner: &'a str, observed: &'a [String]) -> &'a str {
    if !CLASH_FAMILY.contains(&owner) {
        return owner;
    }
    observed
        .iter()
        .find(|label| CLASH_FAMILY.contains(&label.as_str()))
        .map_or(owner, String::as_str)
}

/// Label of a tunnel interface when its name says whose it is.
fn interface_owner(name: &str) -> Option<&'static str> {
    let name = name.to_lowercase();
    [
        ("tailscale", "Tailscale"),
        ("zerotier", "ZeroTier"),
        ("zt", "ZeroTier"),
        ("wireguard", "WireGuard"),
        ("wg", "WireGuard"),
        ("nordlynx", "NordVPN"),
        ("mullvad", "Mullvad"),
        ("expressvpn", "ExpressVPN"),
        ("openvpn", "OpenVPN"),
        ("tap-windows", "OpenVPN"),
        ("mihomo", "Mihomo"),
        ("clash", "Clash"),
        // mihomo's default TUN name (the core formerly called Clash Meta).
        ("meta", "Mihomo"),
        ("sing-box", "sing-box"),
        ("singbox", "sing-box"),
        ("sing-tun", "sing-box"),
        ("xray", "Xray"),
        ("cisco", "Cisco AnyConnect"),
        ("anyconnect", "Cisco AnyConnect"),
        ("pangp", "GlobalProtect"),
        ("zscaler", "Zscaler"),
    ]
    .into_iter()
    .find(|(keyword, _)| name.starts_with(keyword) || (keyword.len() > 3 && name.contains(keyword)))
    .map(|(_, label)| label)
}

/// Label of a tunnel interface when its address says whose it is:
/// Tailscale hands out 100.64.0.0/10 (CGNAT) addresses, so a macOS `utunN`
/// holding one is Tailscale's (e.g. an exit node holding the route).
fn address_owner(interface: &InterfaceInfo) -> Option<&'static str> {
    interface
        .ipv4
        .iter()
        .any(|ip| {
            let [a, b, _, _] = ip.octets();
            a == 100 && (64..128).contains(&b)
        })
        .then_some("Tailscale")
}

/// Label of a tunnel interface, by name first, then by address.
fn tunnel_owner(name: &str, interfaces: &[InterfaceInfo]) -> Option<&'static str> {
    interface_owner(name).or_else(|| {
        interfaces
            .iter()
            .find(|interface| interface.name == name)
            .and_then(address_owner)
    })
}

/// Looks like a tunnel/virtual adapter rather than Wi-Fi or Ethernet.
fn is_tunnel_interface(name: &str) -> bool {
    let name = name.to_lowercase();
    [
        "utun",
        "tun",
        "tap",
        "wg",
        "ppp",
        "ipsec",
        "tailscale",
        "zt",
        "nordlynx",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
        // mihomo's default TUN name on Linux.
        || name == "meta"
        || [
            "wintun",
            "wireguard",
            "tap-windows",
            "tunnel",
            "openvpn",
            "vpn",
            "clash",
            "mihomo",
            "sing-box",
            "singbox",
            "sing-tun",
            "xray",
            "tailscale",
            "zerotier",
        ]
        .iter()
        .any(|keyword| name.contains(keyword))
}

fn is_fake_ip(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a == 198 && (b == 18 || b == 19)
}

fn is_own_interface(interface: &InterfaceInfo) -> bool {
    OWN_TUN_ADDRESSES
        .iter()
        .any(|own| interface.ipv4.contains(own))
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|existing| existing == value) {
        list.push(value.to_string());
    }
}

/// The report for `inputs` (pure; see the module docs for the rule).
pub(crate) fn evaluate(inputs: &Inputs) -> ConflictReport {
    let mut observed = Vec::new();
    let mut tunnel_only = Vec::new();
    for process in &inputs.processes {
        if is_own_process(process, inputs.own_pid) {
            continue;
        }
        if let Some(known) = classify(process) {
            push_unique(&mut observed, known.label);
            if known.tunnel_only {
                push_unique(&mut tunnel_only, known.label);
            }
        }
    }

    let mut fake_ip_interfaces = Vec::new();
    let mut fake_ip_owners: Vec<String> = Vec::new();
    for interface in &inputs.interfaces {
        if is_own_interface(interface) {
            continue;
        }
        if let Some(ip) = interface.ipv4.iter().copied().find(|ip| is_fake_ip(*ip)) {
            fake_ip_interfaces.push(format!("{} ({ip})", interface.name));
            if let Some(owner) = interface_owner(&interface.name) {
                push_unique(&mut fake_ip_owners, owner);
            }
        }
    }

    let own_names: Vec<&str> = inputs
        .interfaces
        .iter()
        .filter(|interface| is_own_interface(interface))
        .map(|interface| interface.name.as_str())
        .collect();
    let foreign = |name: &&str| is_tunnel_interface(name) && !own_names.contains(name);
    // What the routing table picks first; else another app's tunnel still
    // holding a default / split-default route next to ours.
    let foreign_default_route = inputs
        .route_interface
        .as_deref()
        .filter(foreign)
        .or_else(|| {
            inputs
                .default_routes
                .iter()
                .map(String::as_str)
                .find(foreign)
        })
        .map(str::to_string);

    // Fake-IP DNS counts only while our own TUN is down (it is not ours).
    let fake_ip_dns: Vec<String> = if own_names.is_empty() {
        let mut list = Vec::new();
        for ip in inputs.resolvers.iter().filter(|ip| is_fake_ip(**ip)) {
            push_unique(&mut list, &ip.to_string());
        }
        list
    } else {
        Vec::new()
    };

    let route_owner = foreign_default_route
        .as_deref()
        .and_then(|name| tunnel_owner(name, &inputs.interfaces));
    let mut competitors = Vec::new();
    if !fake_ip_interfaces.is_empty() || foreign_default_route.is_some() || !fake_ip_dns.is_empty()
    {
        // The tunnel's own owner first, then the running proxy apps, then
        // tunnel-only apps when they may be the ones holding the route.
        if let Some(owner) = route_owner {
            push_unique(&mut competitors, refine_owner(owner, &observed));
        }
        for owner in &fake_ip_owners {
            push_unique(&mut competitors, refine_owner(owner, &observed));
        }
        for label in observed.iter().filter(|label| !tunnel_only.contains(label)) {
            push_unique(&mut competitors, label);
        }
        // A fake-IP tunnel is a proxy app's, never a mesh VPN's.
        let route_is_fake_ip = foreign_default_route.as_deref().is_some_and(|name| {
            inputs.interfaces.iter().any(|interface| {
                interface.name == name && interface.ipv4.iter().any(|ip| is_fake_ip(*ip))
            })
        });
        if foreign_default_route.is_some() && route_owner.is_none() && !route_is_fake_ip {
            for label in &tunnel_only {
                push_unique(&mut competitors, label);
            }
        }
    }
    ConflictReport {
        competitors,
        observed,
        fake_ip_interfaces,
        foreign_default_route,
        fake_ip_dns,
    }
}

/// Another app's tunnel owns the network right now: a named competitor
/// together with a strong signal (a foreign tunnel holds the route to the
/// internet or a default / split-default route, or the system DNS is in the
/// fake-IP range). Starting our TUN on top of it would fight over routes
/// and DNS, so enhanced mode does not start. A fake-IP interface alone, or
/// a mesh VPN without the default route, is not enough.
pub(crate) fn tunnel_owns_network(report: &ConflictReport) -> bool {
    !report.competitors.is_empty()
        && (report.foreign_default_route.is_some() || !report.fake_ip_dns.is_empty())
}

/// The name to show for a conflict, if any app can be named.
pub(crate) fn display_competitor(report: &ConflictReport) -> Option<String> {
    report.competitors.first().cloned()
}

// ---------------------------------------------------------------------------
// Gathering (blocking; call from spawn_blocking)
// ---------------------------------------------------------------------------

/// Reads processes, interfaces and the internet route of this machine.
pub(crate) fn gather() -> Inputs {
    let mut processes = processes();
    // Stable order (the process table is a hash map).
    processes.sort_by_key(|process| process.pid);
    let (interfaces, route_interface, default_routes, resolvers) = os::network();
    Inputs {
        processes,
        interfaces,
        route_interface,
        default_routes,
        resolvers,
        own_pid: std::process::id(),
    }
}

/// Runs detection on this machine (blocking).
pub(crate) fn detect_now() -> ConflictReport {
    evaluate(&gather())
}

fn processes() -> Vec<ProcessInfo> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new().with_exe(UpdateKind::OnlyIfNotSet),
    );
    system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessInfo {
            pid: pid.as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            exe: process
                .exe()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
        .collect()
}

/// A public address for "which interface carries internet traffic" (no
/// packet is sent; only the routing table is asked).
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
const PROBE_DESTINATION: &str = "1.1.1.1";

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `ifconfig` output → interfaces (macOS / BSD format).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_ifconfig(text: &str) -> Vec<InterfaceInfo> {
    let mut interfaces: Vec<InterfaceInfo> = Vec::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) && line.contains(':') {
            let name = line.split(':').next().unwrap_or_default().to_string();
            interfaces.push(InterfaceInfo {
                name,
                ipv4: Vec::new(),
            });
        } else if let Some(rest) = line.trim().strip_prefix("inet ") {
            if let (Some(interface), Some(ip)) = (
                interfaces.last_mut(),
                rest.split_whitespace()
                    .next()
                    .and_then(|ip| ip.parse::<Ipv4Addr>().ok()),
            ) {
                interface.ipv4.push(ip);
            }
        }
    }
    interfaces
}

/// `route -n get <ip>` output → the interface.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_route_get(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("interface:"))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// `netstat -rn -f inet` output → interfaces holding a default or
/// split-default route (`default`, `0/1`, `128.0/1`), in table order.
/// Interface-scoped routes (flag `I`) do not carry the default traffic and
/// are skipped.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_netstat_routes(text: &str) -> Vec<String> {
    let mut list = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [destination, _gateway, flags, netif, ..] = fields[..] else {
            continue;
        };
        let default = matches!(
            destination,
            "default" | "0/1" | "128.0/1" | "0.0.0.0/1" | "128.0.0.0/1" | "0.0.0.0/0"
        );
        if default && !flags.contains('I') {
            push_unique(&mut list, netif);
        }
    }
    list
}

/// `scutil --dns` output → the IPv4 resolvers.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_scutil_dns(text: &str) -> Vec<Ipv4Addr> {
    let mut list = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("nameserver[") {
            continue;
        }
        if let Some(ip) = line
            .split(':')
            .nth(1)
            .and_then(|value| value.trim().parse::<Ipv4Addr>().ok())
        {
            if !list.contains(&ip) {
                list.push(ip);
            }
        }
    }
    list
}

/// `/etc/resolv.conf` → the IPv4 resolvers.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_resolv_conf(text: &str) -> Vec<Ipv4Addr> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .filter_map(|value| value.trim().parse().ok())
        .collect()
}

/// `ip -j addr` JSON → interfaces.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_ip_addr_json(text: &str) -> Vec<InterfaceInfo> {
    let Ok(serde_json::Value::Array(links)) = serde_json::from_str(text) else {
        return Vec::new();
    };
    links
        .iter()
        .filter_map(|link| {
            let name = link["ifname"].as_str()?.to_string();
            let ipv4 = link["addr_info"]
                .as_array()
                .map(|addresses| {
                    addresses
                        .iter()
                        .filter(|address| address["family"] == "inet")
                        .filter_map(|address| address["local"].as_str()?.parse().ok())
                        .collect()
                })
                .unwrap_or_default();
            Some(InterfaceInfo { name, ipv4 })
        })
        .collect()
}

/// `ip -j route get <ip>` JSON → the device.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_ip_route_json(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value
        .as_array()?
        .first()?
        .get("dev")?
        .as_str()
        .map(str::to_string)
}

/// `/proc/net/route` → the device of the default route with the lowest
/// metric (fallback when `ip` is missing).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_proc_net_route(text: &str) -> Option<String> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.len() > 7 && fields[1] == "00000000" && fields[7] == "00000000")
                .then(|| (fields[6].parse::<u64>().unwrap_or(u64::MAX), fields[0]))
        })
        .min()
        .map(|(_, device)| device.to_string())
}

/// `/proc/net/route` → devices holding a default or split-default route
/// (`0.0.0.0/0`, `0.0.0.0/1`, `128.0.0.0/1`; little-endian hex).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_proc_net_route_defaults(text: &str) -> Vec<String> {
    let mut list = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() <= 7 {
            continue;
        }
        let default = matches!(
            (fields[1], fields[7]),
            ("00000000", "00000000") | ("00000000", "00000080") | ("00000080", "00000080")
        );
        if default {
            push_unique(&mut list, fields[0]);
        }
    }
    list
}

/// One Windows network adapter as IP Helper (`GetAdaptersAddresses`)
/// reports it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug, Default)]
pub(crate) struct WindowsAdapter {
    pub index: u32,
    /// Friendly name ("Wi-Fi", "Clash", "singbox_tun").
    pub alias: String,
    /// Driver description ("Wintun Userspace Tunnel", "TAP-Windows Adapter
    /// V9"): what tells a tunnel apart.
    pub description: String,
    /// `OperStatus == IfOperStatusUp`.
    pub up: bool,
    pub ipv4: Vec<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    /// The interface's IPv4 metric (added to a route's own metric).
    pub metric: u32,
}

/// One IPv4 route (`GetIpForwardTable2`).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug)]
pub(crate) struct WindowsRoute {
    pub index: u32,
    pub prefix: Ipv4Addr,
    pub prefix_len: u8,
    pub metric: u32,
}

/// What detection reads on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug, Default)]
pub(crate) struct WindowsNetwork {
    pub interfaces: Vec<InterfaceInfo>,
    pub route: Option<String>,
    pub default_routes: Vec<String>,
    pub resolvers: Vec<Ipv4Addr>,
}

/// Adapters, routes and the interface of the best route to the probe
/// address → interfaces (named `alias [description]`), the route interface,
/// default / split-default route holders and the DNS servers (pure; the
/// Win32 calls live in `os`). Routes and DNS count on adapters that are up
/// only. Holders are ordered as Windows picks them: the split-default
/// `/1` halves first (longer prefix), then by route + interface metric.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn windows_network(
    adapters: &[WindowsAdapter],
    routes: &[WindowsRoute],
    best_index: Option<u32>,
) -> WindowsNetwork {
    let name = |adapter: &WindowsAdapter| {
        if adapter.description.is_empty() {
            adapter.alias.clone()
        } else {
            format!("{} [{}]", adapter.alias, adapter.description)
        }
    };
    let up = |index: u32| {
        adapters
            .iter()
            .find(|adapter| adapter.index == index && adapter.up)
    };
    let interfaces = adapters
        .iter()
        .map(|adapter| InterfaceInfo {
            name: name(adapter),
            ipv4: adapter.ipv4.clone(),
        })
        .collect();
    let route = best_index
        .and_then(|index| adapters.iter().find(|adapter| adapter.index == index))
        .map(name);
    let mut defaults: Vec<(u8, u64, &WindowsAdapter)> = routes
        .iter()
        .filter(|route| {
            matches!(
                (route.prefix.octets(), route.prefix_len),
                ([0, 0, 0, 0], 0) | ([0, 0, 0, 0], 1) | ([128, 0, 0, 0], 1)
            )
        })
        .filter_map(|route| {
            up(route.index).map(|adapter| {
                (
                    route.prefix_len,
                    u64::from(route.metric) + u64::from(adapter.metric),
                    adapter,
                )
            })
        })
        .collect();
    defaults.sort_by_key(|(len, metric, _)| (std::cmp::Reverse(*len), *metric));
    let mut default_routes = Vec::new();
    for (_, _, adapter) in defaults {
        push_unique(&mut default_routes, &name(adapter));
    }
    let mut resolvers = Vec::new();
    for ip in adapters
        .iter()
        .filter(|adapter| adapter.up)
        .flat_map(|adapter| adapter.dns.iter())
    {
        if !resolvers.contains(ip) {
            resolvers.push(*ip);
        }
    }
    WindowsNetwork {
        interfaces,
        route,
        default_routes,
        resolvers,
    }
}

/// Interfaces, the route interface, default-route holders, resolvers.
type Network = (
    Vec<InterfaceInfo>,
    Option<String>,
    Vec<String>,
    Vec<Ipv4Addr>,
);

#[cfg(target_os = "macos")]
mod os {
    use super::*;

    pub(super) fn network() -> Network {
        let interfaces = command_output("/sbin/ifconfig", &[])
            .map(|text| parse_ifconfig(&text))
            .unwrap_or_default();
        let route = command_output("/sbin/route", &["-n", "get", PROBE_DESTINATION])
            .and_then(|text| parse_route_get(&text));
        let defaults = command_output("/usr/sbin/netstat", &["-rn", "-f", "inet"])
            .map(|text| parse_netstat_routes(&text))
            .unwrap_or_default();
        let resolvers = command_output("/usr/sbin/scutil", &["--dns"])
            .map(|text| parse_scutil_dns(&text))
            .unwrap_or_default();
        (interfaces, route, defaults, resolvers)
    }
}

#[cfg(target_os = "linux")]
mod os {
    use super::*;

    pub(super) fn network() -> Network {
        let interfaces = command_output("ip", &["-j", "addr"])
            .map(|text| parse_ip_addr_json(&text))
            .unwrap_or_default();
        let proc_route = std::fs::read_to_string("/proc/net/route").ok();
        // `ip route get` follows policy rules (Clash / sing-box TUN use their
        // own table); /proc/net/route is the main table only.
        let route = command_output("ip", &["-j", "route", "get", PROBE_DESTINATION])
            .and_then(|text| parse_ip_route_json(&text))
            .or_else(|| proc_route.as_deref().and_then(parse_proc_net_route));
        let defaults = proc_route
            .as_deref()
            .map(parse_proc_net_route_defaults)
            .unwrap_or_default();
        let resolvers = std::fs::read_to_string("/etc/resolv.conf")
            .map(|text| parse_resolv_conf(&text))
            .unwrap_or_default();
        (interfaces, route, defaults, resolvers)
    }
}

#[cfg(windows)]
mod os {
    //! IP Helper instead of PowerShell (13-16 s per query on a VM, 1.4 s
    //! just to start): a few milliseconds.

    use super::*;
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetAdaptersAddresses, GetBestInterface, GetIpForwardTable2,
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
        IP_ADAPTER_ADDRESSES_LH, MIB_IPFORWARD_TABLE2,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR, SOCKADDR_IN, SOCKET_ADDRESS};

    /// `IfOperStatusUp`.
    const OPER_STATUS_UP: i32 = 1;

    pub(super) fn network() -> Network {
        let adapters = adapters();
        let routes = routes();
        let best = best_interface();
        let parsed = windows_network(&adapters, &routes, best);
        (
            parsed.interfaces,
            parsed.route,
            parsed.default_routes,
            parsed.resolvers,
        )
    }

    /// A NUL-terminated UTF-16 string.
    ///
    /// # Safety
    /// `text` is null or points at a NUL-terminated UTF-16 string.
    unsafe fn wide(text: *const u16) -> String {
        if text.is_null() {
            return String::new();
        }
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
    }

    /// The IPv4 address of a `SOCKET_ADDRESS`, if it is one.
    ///
    /// # Safety
    /// `address.lpSockaddr` is null or points at `iSockaddrLength` bytes.
    unsafe fn ipv4_of(address: &SOCKET_ADDRESS) -> Option<Ipv4Addr> {
        let sockaddr: *const SOCKADDR = address.lpSockaddr;
        if sockaddr.is_null()
            || (address.iSockaddrLength as usize) < std::mem::size_of::<SOCKADDR_IN>()
            || (*sockaddr).sa_family != AF_INET
        {
            return None;
        }
        let v4 = &*(sockaddr as *const SOCKADDR_IN);
        // `S_addr` is in network byte order: its bytes are the octets.
        Some(Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes()))
    }

    fn adapters() -> Vec<WindowsAdapter> {
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
        // 15 KB is Microsoft's suggested start; u64 keeps the list aligned.
        let mut size: u32 = 15 * 1024;
        let mut buffer: Vec<u64> = Vec::new();
        let mut result = ERROR_BUFFER_OVERFLOW;
        for _ in 0..3 {
            buffer = vec![0u64; (size as usize).div_ceil(8)];
            // SAFETY: `buffer` holds at least `size` bytes, 8-byte aligned.
            result = unsafe {
                GetAdaptersAddresses(
                    u32::from(AF_INET),
                    flags,
                    std::ptr::null(),
                    buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            if result != ERROR_BUFFER_OVERFLOW {
                break;
            }
        }
        if result != NO_ERROR {
            tracing::debug!("GetAdaptersAddresses failed: {result}");
            return Vec::new();
        }
        let mut list = Vec::new();
        let mut current = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: on success the buffer holds a linked list of adapters whose
        // pointers stay valid while `buffer` lives.
        unsafe {
            while !current.is_null() {
                let adapter = &*current;
                let mut ipv4 = Vec::new();
                let mut unicast = adapter.FirstUnicastAddress;
                while !unicast.is_null() {
                    if let Some(ip) = ipv4_of(&(*unicast).Address) {
                        ipv4.push(ip);
                    }
                    unicast = (*unicast).Next;
                }
                let mut dns = Vec::new();
                let mut server = adapter.FirstDnsServerAddress;
                while !server.is_null() {
                    if let Some(ip) = ipv4_of(&(*server).Address) {
                        dns.push(ip);
                    }
                    server = (*server).Next;
                }
                list.push(WindowsAdapter {
                    index: adapter.Anonymous1.Anonymous.IfIndex,
                    alias: wide(adapter.FriendlyName),
                    description: wide(adapter.Description),
                    up: adapter.OperStatus == OPER_STATUS_UP,
                    ipv4,
                    dns,
                    metric: adapter.Ipv4Metric,
                });
                current = adapter.Next;
            }
        }
        list
    }

    fn routes() -> Vec<WindowsRoute> {
        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        // SAFETY: on success `table` points at a table freed below.
        let result = unsafe { GetIpForwardTable2(AF_INET, &mut table) };
        if result != NO_ERROR || table.is_null() {
            tracing::debug!("GetIpForwardTable2 failed: {result}");
            return Vec::new();
        }
        let mut list = Vec::new();
        // SAFETY: `Table` is a flexible array of `NumEntries` rows.
        unsafe {
            let rows =
                std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
            for row in rows {
                let prefix = &row.DestinationPrefix;
                if prefix.Prefix.si_family != AF_INET {
                    continue;
                }
                let raw = prefix.Prefix.Ipv4.sin_addr.S_un.S_addr;
                list.push(WindowsRoute {
                    index: row.InterfaceIndex,
                    prefix: Ipv4Addr::from(raw.to_ne_bytes()),
                    prefix_len: prefix.PrefixLength,
                    metric: row.Metric,
                });
            }
            FreeMibTable(table.cast());
        }
        list
    }

    /// The interface Windows picks for the probe address.
    fn best_interface() -> Option<u32> {
        let destination: Ipv4Addr = PROBE_DESTINATION.parse().ok()?;
        let mut index = 0u32;
        // SAFETY: plain value in, one u32 out. The address is in network
        // byte order: its octets as laid out in memory.
        let result =
            unsafe { GetBestInterface(u32::from_ne_bytes(destination.octets()), &mut index) };
        (result == NO_ERROR).then_some(index)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod os {
    pub(super) fn network() -> super::Network {
        Default::default()
    }
}

/// The process listening on loopback `port`, by display name when it is a
/// known app (best effort; `None` when it cannot be told).
pub(crate) fn loopback_listener(port: u16) -> Option<String> {
    let pid = listener_pid(port)?;
    let processes = processes();
    let process = processes.iter().find(|process| process.pid == pid)?;
    Some(
        classify(process)
            .map(|known| known.label.to_string())
            .unwrap_or_else(|| process.name.clone()),
    )
}

#[cfg(target_os = "macos")]
fn listener_pid(port: u16) -> Option<u32> {
    let text = command_output(
        "/usr/sbin/lsof",
        &["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp"],
    )?;
    text.lines()
        .find_map(|line| line.strip_prefix('p'))
        .and_then(|pid| pid.parse().ok())
}

#[cfg(target_os = "linux")]
fn listener_pid(port: u16) -> Option<u32> {
    let text = command_output("ss", &["-ltnpH", &format!("sport = :{port}")])?;
    let start = text.find("pid=")? + 4;
    text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

#[cfg(windows)]
fn listener_pid(port: u16) -> Option<u32> {
    let script = format!(
        "(Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1).OwningProcess"
    );
    command_output(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )?
    .trim()
    .parse()
    .ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn listener_pid(_: u16) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, exe: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.into(),
            exe: exe.into(),
        }
    }

    fn interface(name: &str, ips: &[&str]) -> InterfaceInfo {
        InterfaceInfo {
            name: name.into(),
            ipv4: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
        }
    }

    fn base() -> Inputs {
        Inputs {
            processes: vec![
                process(10, "mihomo", "/usr/local/bin/mihomo"),
                process(11, "Finder", "/System/Finder"),
                process(12, "tailscaled", "/usr/sbin/tailscaled"),
            ],
            interfaces: vec![interface("en0", &["192.168.1.20"])],
            route_interface: Some("en0".into()),
            default_routes: vec!["en0".into()],
            resolvers: vec![Ipv4Addr::new(192, 168, 1, 1)],
            own_pid: 1,
        }
    }

    #[test]
    fn running_apps_alone_are_no_conflict() {
        let report = evaluate(&base());
        assert!(report.competitors.is_empty());
        assert_eq!(report.observed, vec!["Mihomo", "Tailscale"]);
        assert!(report.fake_ip_interfaces.is_empty());
        assert_eq!(report.foreign_default_route, None);
        assert_eq!(display_competitor(&report), None);
    }

    #[test]
    fn a_fake_ip_tunnel_is_a_conflict() {
        let mut inputs = base();
        inputs.interfaces.push(interface("utun5", &["198.18.0.1"]));
        let report = evaluate(&inputs);
        assert_eq!(report.fake_ip_interfaces, vec!["utun5 (198.18.0.1)"]);
        // Tailscale runs but does not hold the route: not a competitor.
        assert_eq!(report.competitors, vec!["Mihomo"]);
        assert_eq!(display_competitor(&report).as_deref(), Some("Mihomo"));
    }

    #[test]
    fn a_foreign_default_route_is_a_conflict() {
        let mut inputs = base();
        inputs.interfaces.push(interface("utun7", &["10.8.0.2"]));
        inputs.route_interface = Some("utun7".into());
        let report = evaluate(&inputs);
        assert_eq!(report.foreign_default_route.as_deref(), Some("utun7"));
        // Holding the route, a tunnel-only app counts too.
        assert_eq!(report.competitors, vec!["Mihomo", "Tailscale"]);

        // Named by the interface when no process says who it is.
        let inputs = Inputs {
            processes: Vec::new(),
            interfaces: vec![interface("tailscale0", &["100.64.0.1"])],
            route_interface: Some("tailscale0".into()),
            ..Inputs::default()
        };
        assert_eq!(evaluate(&inputs).competitors, vec!["Tailscale"]);
    }

    #[test]
    fn our_own_tunnel_and_processes_are_excluded() {
        let inputs = Inputs {
            processes: vec![
                process(1, "PPVPN", "/Applications/PPVPN.app/Contents/MacOS/PPVPN"),
                process(
                    2,
                    "ppvpn-core",
                    "/Applications/PPVPN.app/Contents/MacOS/ppvpn-core",
                ),
                process(
                    3,
                    "ppvpn-service",
                    "/Library/PrivilegedHelperTools/ppvpn-service",
                ),
            ],
            interfaces: vec![
                interface("en0", &["192.168.1.20"]),
                interface("utun4", &["10.60.159.89"]),
            ],
            route_interface: Some("utun4".into()),
            default_routes: vec!["utun4".into(), "en0".into()],
            // Our TUN is up: a fake-IP resolver is not counted.
            resolvers: vec![Ipv4Addr::new(198, 18, 0, 2)],
            own_pid: 1,
        };
        let report = evaluate(&inputs);
        assert_eq!(report, ConflictReport::default());

        // Our TUN with a fake-IP alias is still ours.
        let mut inputs = inputs;
        inputs.interfaces[1]
            .ipv4
            .push("198.18.0.1".parse().unwrap());
        assert!(evaluate(&inputs).fake_ip_interfaces.is_empty());
    }

    #[test]
    fn ordinary_interfaces_holding_the_route_are_fine() {
        for name in ["en0", "eth0", "wlp3s0", "Wi-Fi [Intel(R) Wi-Fi 6 AX201]"] {
            let inputs = Inputs {
                route_interface: Some(name.into()),
                ..base()
            };
            assert_eq!(evaluate(&inputs).foreign_default_route, None, "{name}");
        }
        for name in [
            "utun3",
            "wg0",
            "tun0",
            "ppp0",
            "Ethernet 3 [Wintun Userspace Tunnel]",
        ] {
            assert!(is_tunnel_interface(name), "{name}");
        }
    }

    #[test]
    fn os_listings_parse() {
        let ifconfig = "lo0: flags=8049<UP,LOOPBACK> mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\nen0: flags=8863<UP> mtu 1500\n\tinet6 fe80::1%en0 prefixlen 64\n\tinet 192.168.1.20 netmask 0xffffff00 broadcast 192.168.1.255\nutun5: flags=8051<UP> mtu 1500\n\tinet 198.18.0.1 --> 198.18.0.1 netmask 0xffff0000\n";
        let interfaces = parse_ifconfig(ifconfig);
        assert_eq!(interfaces.len(), 3);
        assert_eq!(interfaces[2].name, "utun5");
        assert_eq!(interfaces[2].ipv4, vec![Ipv4Addr::new(198, 18, 0, 1)]);
        assert_eq!(
            parse_route_get("   route to: 1.1.1.1\ndestination: default\n  interface: utun5\n")
                .as_deref(),
            Some("utun5")
        );
        let ip = r#"[{"ifindex":1,"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1"}]},{"ifname":"Meta","addr_info":[{"family":"inet","local":"198.18.0.1"},{"family":"inet6","local":"fdfe::1"}]}]"#;
        let interfaces = parse_ip_addr_json(ip);
        assert_eq!(interfaces[1].name, "Meta");
        assert_eq!(interfaces[1].ipv4, vec![Ipv4Addr::new(198, 18, 0, 1)]);
        assert_eq!(
            parse_ip_route_json(r#"[{"dst":"1.1.1.1","dev":"wg0","prefsrc":"10.0.0.2"}]"#)
                .as_deref(),
            Some("wg0")
        );
        let proc_route = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
            eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\n\
            wg0\t00000000\t00000000\t0001\t0\t0\t50\t00000000\n\
            eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\n";
        assert_eq!(parse_proc_net_route(proc_route).as_deref(), Some("wg0"));
        let network = windows_network(
            &[
                win_adapter(1, "Wi-Fi", "Intel(R) Wi-Fi 6", &["192.168.1.5"]),
                win_adapter(2, "Clash", "Wintun Userspace Tunnel", &["198.18.0.1"]),
            ],
            &[],
            Some(2),
        );
        assert_eq!(
            network.interfaces[1].name,
            "Clash [Wintun Userspace Tunnel]"
        );
        assert_eq!(
            network.route.as_deref(),
            Some("Clash [Wintun Userspace Tunnel]")
        );
        let report = evaluate(&Inputs {
            interfaces: network.interfaces,
            route_interface: network.route,
            ..Inputs::default()
        });
        assert_eq!(report.competitors, vec!["Clash"]);
    }

    /// The real case: Surge's enhanced mode on macOS (a utun holding
    /// 198.18.0.1, split-default routes through it, DNS 198.18.0.2).
    const SURGE_IFCONFIG: &str = "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\
\toptions=1203<RXCSUM,TXCSUM,TXSTATUS,SW_TIMESTAMP>\n\
\tinet 127.0.0.1 netmask 0xff000000\n\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
\tether 3c:22:fb:00:00:01\n\
\tinet 192.168.1.20 netmask 0xffffff00 broadcast 192.168.1.255\n\
utun0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380\n\
\tinet6 fe80::1%utun0 prefixlen 64 scopeid 0x10\n\
utun5: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 4064\n\
\tinet 198.18.0.1 --> 198.18.0.1 netmask 0xffff0000\n";

    const SURGE_NETSTAT: &str = "Routing tables\n\
\n\
Internet:\n\
Destination        Gateway            Flags               Netif Expire\n\
0/1                198.18.0.1         UGSc                utun5\n\
default            192.168.1.1        UGScg                 en0\n\
default            link#16            UCSIg               utun0\n\
127                127.0.0.1          UCS                   lo0\n\
128.0/1            198.18.0.1         UGSc                utun5\n\
192.168.1          link#11            UCS                   en0      !\n\
198.18/15          198.18.0.1         UGSc                utun5\n";

    const SURGE_SCUTIL: &str = "DNS configuration\n\
\n\
resolver #1\n\
  nameserver[0] : 198.18.0.2\n\
  if_index : 16 (utun5)\n\
  flags    : Request A records\n\
\n\
resolver #2\n\
  domain   : local\n\
  options  : mdns\n";

    fn surge_inputs() -> Inputs {
        Inputs {
            processes: vec![
                process(300, "Surge", "/Applications/Surge.app/Contents/MacOS/Surge"),
                process(
                    301,
                    "com.nssurge.surge-mac.helper",
                    "/Library/PrivilegedHelperTools/com.nssurge.surge-mac.helper",
                ),
                process(302, "tailscaled", "/usr/local/bin/tailscaled"),
            ],
            interfaces: parse_ifconfig(SURGE_IFCONFIG),
            route_interface: parse_route_get(
                "   route to: 1.1.1.1\ndestination: 0.0.0.0\n       mask: 128.0.0.0\n    gateway: 198.18.0.1\n  interface: utun5\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING>\n",
            ),
            default_routes: parse_netstat_routes(SURGE_NETSTAT),
            resolvers: parse_scutil_dns(SURGE_SCUTIL),
            own_pid: 1,
        }
    }

    #[test]
    fn macos_listings_of_surge_parse() {
        let inputs = surge_inputs();
        assert_eq!(inputs.route_interface.as_deref(), Some("utun5"));
        // The interface-scoped system default on utun0 is skipped.
        assert_eq!(inputs.default_routes, vec!["utun5", "en0"]);
        assert_eq!(inputs.resolvers, vec![Ipv4Addr::new(198, 18, 0, 2)]);
        assert_eq!(
            parse_netstat_routes("default  192.168.1.1  UGScg  en0\n"),
            vec!["en0"]
        );
        assert!(parse_netstat_routes("Destination Gateway Flags Netif Expire\n").is_empty());
    }

    // Surge is in the macOS app table only.
    #[cfg(target_os = "macos")]
    #[test]
    fn surge_enhanced_mode_is_named_on_macos() {
        let report = evaluate(&surge_inputs());
        assert_eq!(report.foreign_default_route.as_deref(), Some("utun5"));
        assert_eq!(report.fake_ip_interfaces, vec!["utun5 (198.18.0.1)"]);
        assert_eq!(report.fake_ip_dns, vec!["198.18.0.2"]);
        // Surge, not Tailscale: a fake-IP tunnel is never a mesh VPN's.
        assert_eq!(report.competitors, vec!["Surge"]);
        assert_eq!(display_competitor(&report).as_deref(), Some("Surge"));
    }

    // Surge is in the macOS app table only.
    #[cfg(target_os = "macos")]
    #[test]
    fn surge_is_named_while_our_tun_wins_the_route() {
        // Our TUN came up too and `route get` picks it, but Surge's utun
        // still holds the split-default routes.
        let mut inputs = surge_inputs();
        inputs.interfaces.push(interface("utun6", &["172.19.0.1"])); // a core before 0.5.7
        inputs.route_interface = Some("utun6".into());
        inputs.default_routes.insert(0, "utun6".into());
        let report = evaluate(&inputs);
        assert_eq!(report.foreign_default_route.as_deref(), Some("utun5"));
        // Our TUN is up: the resolver may be ours.
        assert!(report.fake_ip_dns.is_empty());
        assert_eq!(report.competitors, vec!["Surge"]);

        // Only the routes say it (no fake-IP address on the tunnel).
        let mut inputs = surge_inputs();
        inputs
            .interfaces
            .retain(|interface| interface.name != "utun5");
        inputs.interfaces.push(interface("utun5", &["10.0.85.1"]));
        inputs.route_interface = Some("en0".into());
        inputs.resolvers.clear();
        let report = evaluate(&inputs);
        assert_eq!(report.foreign_default_route.as_deref(), Some("utun5"));
        assert_eq!(report.competitors, vec!["Surge", "Tailscale"]);
    }

    #[test]
    fn fake_ip_dns_alone_is_a_conflict() {
        let inputs = Inputs {
            resolvers: vec![Ipv4Addr::new(198, 18, 0, 2)],
            ..base()
        };
        let report = evaluate(&inputs);
        assert_eq!(report.fake_ip_dns, vec!["198.18.0.2"]);
        assert!(report.fake_ip_interfaces.is_empty());
        assert_eq!(report.foreign_default_route, None);
        // Tailscale does not hold the route: only the proxy app.
        assert_eq!(report.competitors, vec!["Mihomo"]);
    }

    #[test]
    fn a_tailscale_exit_node_is_named_by_its_address() {
        // macOS: Tailscale's utun has no telling name, only a CGNAT address.
        let inputs = Inputs {
            processes: vec![
                process(50, "Surge", "/Applications/Surge.app/Contents/MacOS/Surge"),
                process(
                    51,
                    "Tailscale",
                    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
                ),
            ],
            interfaces: vec![
                interface("en0", &["192.168.1.20"]),
                interface("utun3", &["100.101.102.103"]),
            ],
            route_interface: Some("utun3".into()),
            default_routes: vec!["utun3".into(), "en0".into()],
            ..Inputs::default()
        };
        let report = evaluate(&inputs);
        assert_eq!(
            report.competitors.first().map(String::as_str),
            Some("Tailscale")
        );
    }

    #[test]
    fn linux_listings_parse() {
        let proc_route = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
            eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\n\
            Meta\t00000000\t00000000\t0001\t0\t0\t0\t00000080\n\
            Meta\t00000080\t00000000\t0001\t0\t0\t0\t00000080\n\
            eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\n";
        assert_eq!(
            parse_proc_net_route_defaults(proc_route),
            vec!["eth0", "Meta"]
        );
        assert_eq!(
            parse_resolv_conf("# generated\nnameserver 198.18.0.2\nnameserver ::1\nsearch lan\n"),
            vec![Ipv4Addr::new(198, 18, 0, 2)]
        );
        // mihomo's TUN "Meta" holding the split-default routes, `ip route
        // get` says eth0 (policy table not consulted by the fallback).
        let report = evaluate(&Inputs {
            processes: vec![process(60, "mihomo", "/usr/bin/mihomo")],
            interfaces: vec![
                interface("eth0", &["192.168.1.5"]),
                interface("Meta", &["28.0.0.1"]),
            ],
            route_interface: Some("eth0".into()),
            default_routes: parse_proc_net_route_defaults(proc_route),
            ..Inputs::default()
        });
        assert_eq!(report.foreign_default_route.as_deref(), Some("Meta"));
        // One mihomo process with its "Meta" TUN: named once, as Mihomo.
        assert_eq!(report.competitors, vec!["Mihomo"]);
        assert!(tunnel_owns_network(&report));
    }

    fn win_adapter(index: u32, alias: &str, description: &str, ips: &[&str]) -> WindowsAdapter {
        WindowsAdapter {
            index,
            alias: alias.into(),
            description: description.into(),
            up: true,
            ipv4: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            dns: Vec::new(),
            metric: 25,
        }
    }

    fn win_route(index: u32, prefix: &str, prefix_len: u8, metric: u32) -> WindowsRoute {
        WindowsRoute {
            index,
            prefix: prefix.parse().unwrap(),
            prefix_len,
            metric,
        }
    }

    fn wifi_up_only() -> WindowsAdapter {
        win_adapter(7, "Wi-Fi", "", &["192.168.1.5"])
    }

    #[test]
    fn windows_listing_with_defaults_and_dns_parses() {
        let mut wifi = win_adapter(7, "Wi-Fi", "Intel(R) Wi-Fi 6", &["192.168.1.5"]);
        wifi.dns = vec![Ipv4Addr::new(192, 168, 1, 1)];
        let mut tun = win_adapter(31, "singbox_tun", "sing-tun Tunnel", &["172.18.0.1"]);
        tun.dns = vec![Ipv4Addr::new(198, 18, 0, 2)];
        tun.metric = 0;
        let tap = WindowsAdapter {
            up: false,
            dns: vec![Ipv4Addr::new(10, 8, 0, 1)],
            ..win_adapter(12, "Local Area Connection", "TAP-Windows Adapter V9", &[])
        };
        let routes = [
            win_route(7, "0.0.0.0", 0, 0),
            win_route(7, "192.168.1.0", 24, 256),
            win_route(31, "0.0.0.0", 1, 0),
            win_route(31, "128.0.0.0", 1, 0),
            // A disconnected adapter's persistent default route.
            win_route(12, "0.0.0.0", 0, 0),
        ];
        let network = windows_network(&[wifi, tun, tap], &routes, Some(7));
        assert_eq!(network.route.as_deref(), Some("Wi-Fi [Intel(R) Wi-Fi 6]"));
        // The split-default halves win over 0/0, whatever the metrics.
        assert_eq!(
            network.default_routes,
            vec!["singbox_tun [sing-tun Tunnel]", "Wi-Fi [Intel(R) Wi-Fi 6]"]
        );
        // DNS of adapters that are up, in adapter order.
        assert_eq!(
            network.resolvers,
            vec![Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(198, 18, 0, 2)]
        );
        assert!(is_tunnel_interface(
            "Local Area Connection [TAP-Windows Adapter V9]"
        ));
        // Two full defaults: the lower route + interface metric first.
        let ranked = windows_network(
            &[
                win_adapter(1, "Ethernet", "Intel(R) Ethernet", &["10.0.0.5"]),
                win_adapter(2, "Clash", "Wintun Userspace Tunnel", &["198.18.0.1"]),
            ],
            &[
                win_route(1, "0.0.0.0", 0, 10),
                win_route(2, "0.0.0.0", 0, 0),
            ],
            None,
        );
        assert_eq!(ranked.route, None);
        assert_eq!(
            ranked.default_routes,
            vec![
                "Clash [Wintun Userspace Tunnel]",
                "Ethernet [Intel(R) Ethernet]"
            ]
        );
        // No description: the alias alone.
        let plain = windows_network(&[wifi_up_only()], &[], Some(7));
        assert_eq!(plain.interfaces[0].name, "Wi-Fi");
        // v2rayN running its sing-box TUN.
        let report = evaluate(&Inputs {
            processes: vec![process(70, "v2rayN.exe", "C:\\v2rayN\\v2rayN.exe")],
            interfaces: network.interfaces,
            route_interface: network.route,
            default_routes: network.default_routes,
            resolvers: network.resolvers,
            ..Inputs::default()
        });
        assert_eq!(
            report.foreign_default_route.as_deref(),
            Some("singbox_tun [sing-tun Tunnel]")
        );
        assert_eq!(
            report.competitors.first().map(String::as_str),
            Some("sing-box")
        );
        #[cfg(windows)]
        assert!(report.competitors.contains(&"v2rayN".to_string()));
    }

    /// Runs the real OS readers once (`cargo test -- --ignored live`).
    #[test]
    #[ignore]
    fn live_detection_runs() {
        let inputs = gather();
        println!(
            "processes {}, interfaces {:?}, route {:?}",
            inputs.processes.len(),
            inputs
                .interfaces
                .iter()
                .map(|i| &i.name)
                .collect::<Vec<_>>(),
            inputs.route_interface
        );
        println!("{:?}", evaluate(&inputs));
    }

    #[test]
    fn the_most_specific_name_wins() {
        let label = |name: &str, exe: &str| classify(&process(5, name, exe)).map(|k| k.label);
        // The binary's own name decides, not the directory it lives in.
        assert_eq!(label("mihomo", "/opt/clash-meta/mihomo"), Some("Mihomo"));
        assert_eq!(
            label("clash-meta", "/usr/local/mihomo/clash-meta"),
            Some("Clash Meta")
        );
        assert_eq!(label("", "/usr/local/bin/clash-meta"), Some("Clash Meta"));
        // A generic name falls back to the install path.
        assert_eq!(
            label("core", "/opt/clash-verge/resources/core"),
            Some("Clash Verge")
        );

        // mihomo's TUN is "Meta": the running binary names it.
        let meta = |name: &str, exe: &str| {
            evaluate(&Inputs {
                processes: vec![process(60, name, exe)],
                interfaces: vec![
                    interface("eth0", &["192.168.1.5"]),
                    interface("Meta", &["198.18.0.1"]),
                ],
                route_interface: Some("Meta".into()),
                default_routes: vec!["Meta".into(), "eth0".into()],
                ..Inputs::default()
            })
            .competitors
        };
        assert_eq!(meta("mihomo", "/usr/bin/mihomo"), vec!["Mihomo"]);
        assert_eq!(
            meta("clash-meta", "/usr/bin/clash-meta"),
            vec!["Clash Meta"]
        );
        assert_eq!(
            meta("verge-mihomo", "/usr/bin/verge-mihomo"),
            vec!["Clash Verge"]
        );
        // No process to ask: the core's current name.
        assert_eq!(meta("Finder", "/System/Finder"), vec!["Mihomo"]);
        // Another family's app is not renamed after the tunnel.
        assert_eq!(
            meta("sing-box", "/usr/bin/sing-box"),
            vec!["Mihomo", "sing-box"]
        );
    }

    #[test]
    fn only_a_tunnel_owning_the_network_blocks_our_tun() {
        // A foreign tunnel holding the route, with the app named.
        let mut inputs = base();
        inputs.interfaces.push(interface("utun7", &["10.8.0.2"]));
        inputs.route_interface = Some("utun7".into());
        assert!(tunnel_owns_network(&evaluate(&inputs)));
        // Fake-IP DNS.
        let inputs = Inputs {
            resolvers: vec![Ipv4Addr::new(198, 18, 0, 2)],
            ..base()
        };
        assert!(tunnel_owns_network(&evaluate(&inputs)));
        // A fake-IP interface without the route or DNS is reported, not blocking.
        let mut inputs = base();
        inputs.interfaces.push(interface("utun5", &["198.18.0.1"]));
        let report = evaluate(&inputs);
        assert!(!report.competitors.is_empty());
        assert!(!tunnel_owns_network(&report));
        // Tailscale / ZeroTier without the default route.
        let inputs = Inputs {
            processes: vec![
                process(12, "tailscaled", "/usr/sbin/tailscaled"),
                process(13, "zerotier-one", "/usr/sbin/zerotier-one"),
            ],
            interfaces: vec![
                interface("eth0", &["192.168.1.5"]),
                interface("tailscale0", &["100.64.0.1"]),
                interface("ztabcdef", &["10.147.17.5"]),
            ],
            route_interface: Some("eth0".into()),
            default_routes: vec!["eth0".into()],
            resolvers: vec![Ipv4Addr::new(100, 100, 100, 100)],
            ..Inputs::default()
        };
        let report = evaluate(&inputs);
        assert!(report.competitors.is_empty());
        assert!(!tunnel_owns_network(&report));
        assert!(!tunnel_owns_network(&ConflictReport::default()));
    }

    #[test]
    fn keyword_table_names_apps() {
        let label = |name: &str, exe: &str| classify(&process(5, name, exe)).map(|k| k.label);
        assert_eq!(
            label("clash-verge", "/usr/bin/clash-verge"),
            Some("Clash Verge")
        );
        assert_eq!(label("verge-mihomo", ""), Some("Clash Verge"));
        assert_eq!(label("NordVPN", ""), Some("NordVPN"));
        assert_eq!(label("nekoray", ""), Some("NekoRay"));
        assert_eq!(label("Hiddify", ""), Some("Hiddify"));
        assert_eq!(label("zerotier-one", ""), Some("ZeroTier"));
        assert_eq!(label("clash", "/usr/local/bin/clash"), Some("Clash"));
        assert_eq!(label("xray", ""), Some("Xray"));
        assert_eq!(label("FlClash", ""), Some("FlClash"));
        assert_eq!(label("Clash Verge", ""), Some("Clash Verge"));
        assert_eq!(label("sing-box", ""), Some("sing-box"));
        assert_eq!(label("tailscaled", ""), Some("Tailscale"));
        // Whole names only: not "clashx" on Linux / Windows.
        assert_eq!(label("clashy", ""), None);
        assert_eq!(label("Finder", "/System/Finder"), None);
        #[cfg(target_os = "macos")]
        {
            assert_eq!(label("Surge", ""), Some("Surge"));
            assert_eq!(
                label("Stash", "/Applications/Stash.app/Contents/MacOS/Stash"),
                Some("Stash")
            );
            assert_eq!(label("Quantumult X", ""), Some("Quantumult X"));
            assert_eq!(label("Loon", ""), Some("Loon"));
            assert_eq!(label("Shadowrocket", ""), Some("Shadowrocket"));
            assert_eq!(label("vpnagentd", ""), Some("Cisco AnyConnect"));
            assert_eq!(label("ClashX Meta", ""), Some("ClashX"));
            assert_eq!(label("V2rayU", ""), Some("V2RayU"));
            assert_eq!(label("SFM", ""), Some("sing-box"));
            // Short names only as whole names.
            assert_eq!(label("balloon", ""), None);
        }
    }
}
