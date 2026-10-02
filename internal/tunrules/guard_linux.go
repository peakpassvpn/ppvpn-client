//go:build linux

package tunrules

import (
	"errors"
	"fmt"
	"net"
	"net/netip"
	"os"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/sagernet/netlink"
	"github.com/sagernet/netlink/nl"
	"golang.org/x/sys/unix"
)

// Timing; variables for tests.
var (
	// settle merges a burst of deletions (networkd drops the rules one by
	// one) into one check.
	settle = 50 * time.Millisecond
	// periodic is the backstop for notifications lost to a full socket
	// buffer (ENOBUFS) or never sent.
	periodic = 30 * time.Second
	// flapWindow/flapLimit: more restores than this in the window is logged
	// as an error (something keeps deleting them); restoring goes on.
	flapWindow = 10 * time.Second
	flapLimit  = 10
)

// restoreDisabled turns the restoring off, so a test can prove it is what
// brings the rules back.
var restoreDisabled = os.Getenv("PPVPN_TEST_TUN_RULES_NO_RESTORE") == "1"

type Logger interface {
	Info(msg string, fields ...any)
	Warn(msg string, fields ...any)
	Error(msg string, fields ...any)
}

// Guard watches the scope and puts back what goes missing from the snapshot.
type Guard struct {
	scope    Scope
	link     int
	log      Logger
	onChange func(State)

	rules  []Rule
	routes []Route

	socket   *nl.NetlinkSocket
	requests chan request
	done     chan struct{}
	wg       sync.WaitGroup

	// Owned by the worker goroutine.
	broken   bool
	restores []time.Time
}

type request struct {
	reason string
	by     string
	delay  time.Duration
}

// Start snapshots the scope (call it right after the TUN started) and
// starts watching. onChange is called, from the guard's goroutine, when the
// routing breaks (State.Broken) and when it is back. A nil Guard (no error)
// means sing-tun installed nothing to guard.
func Start(scope Scope, log Logger, onChange func(State)) (*Guard, error) {
	link, err := netlink.LinkByName(scope.Interface)
	if err != nil {
		return nil, fmt.Errorf("find %s: %w", scope.Interface, err)
	}
	listed, err := listRules()
	if err != nil {
		return nil, err
	}
	owned, foreign := scope.Owned(listed)
	for _, r := range foreign {
		log.Info("tun routing rule not ours", "rule", r.String())
	}
	if len(owned) == 0 {
		return nil, nil
	}
	routes, err := listRoutes(scope.Table, link.Attrs().Index)
	if err != nil {
		return nil, err
	}
	socket, err := nl.Subscribe(unix.NETLINK_ROUTE, unix.RTNLGRP_IPV4_RULE, unix.RTNLGRP_IPV6_RULE, unix.RTNLGRP_IPV4_ROUTE, unix.RTNLGRP_IPV6_ROUTE)
	if err != nil {
		return nil, fmt.Errorf("subscribe to routing changes: %w", err)
	}
	g := &Guard{scope: scope, link: link.Attrs().Index, log: log, onChange: onChange, rules: owned, routes: routes,
		socket: socket, requests: make(chan request, 16), done: make(chan struct{})}
	g.wg.Add(2)
	go g.receive()
	go g.work()
	return g, nil
}

// Check asks for a check now (after a kernel switch, a default interface
// change).
func (g *Guard) Check(reason string) {
	if g == nil {
		return
	}
	g.request(request{reason: reason})
}

func (g *Guard) request(r request) {
	select {
	case g.requests <- r:
	case <-g.done:
	default:
		// A check is already queued; it sees the same state.
	}
}

// Close stops the guard. Call it before the TUN closes: sing-tun's own
// cleanup must not be undone.
func (g *Guard) Close() {
	if g == nil {
		return
	}
	select {
	case <-g.done:
		return
	default:
	}
	close(g.done)
	g.socket.Close()
	g.wg.Wait()
}

// receive turns deletions in the scope into checks.
func (g *Guard) receive() {
	defer g.wg.Done()
	for {
		msgs, _, err := g.socket.Receive()
		select {
		case <-g.done:
			return
		default:
		}
		if err != nil {
			if errors.Is(err, unix.ENOBUFS) {
				g.request(request{reason: "notifications lost", delay: settle})
				continue
			}
			if errors.Is(err, os.ErrClosed) || errors.Is(err, unix.EBADF) {
				return
			}
			g.log.Warn("tun routing watch", "error", err)
			time.Sleep(time.Second)
			continue
		}
		for _, m := range msgs {
			if reason, ok := g.relevant(m); ok {
				g.request(request{reason: reason, by: sender(m.Header.Pid), delay: settle})
			}
		}
	}
}

func (g *Guard) relevant(m syscall.NetlinkMessage) (string, bool) {
	switch m.Header.Type {
	case unix.RTM_DELRULE:
		rule, err := parseRule(m.Data)
		if err == nil && g.scope.inRange(rule.Priority) {
			return "rule deleted", true
		}
	case unix.RTM_DELROUTE:
		if table, ok := routeTable(m.Data); ok && table == g.scope.Table {
			return "route deleted", true
		}
	}
	return "", false
}

// sender names the process behind a notification's netlink port id. The
// id is not a pid: socket-activated daemons (systemd-networkd) use a socket
// PID 1 created. So the socket is found by port id in /proc/net/netlink and
// its holders by inode; a holder other than PID 1 is preferred. Best effort:
// "unknown" when nothing matches.
func sender(portID uint32) string {
	if portID == 0 {
		return "kernel"
	}
	inode := netlinkInode(portID)
	if inode == "" {
		return "unknown"
	}
	target := "socket:[" + inode + "]"
	var holders []string
	pids, _ := os.ReadDir("/proc")
	for _, entry := range pids {
		pid := entry.Name()
		if pid[0] < '0' || pid[0] > '9' {
			continue
		}
		fds, err := os.ReadDir("/proc/" + pid + "/fd")
		if err != nil {
			continue
		}
		for _, fd := range fds {
			if link, err := os.Readlink("/proc/" + pid + "/fd/" + fd.Name()); err == nil && link == target {
				comm, _ := os.ReadFile("/proc/" + pid + "/comm")
				name := strings.TrimSpace(string(comm)) + " (pid " + pid + ")"
				if pid == "1" {
					holders = append(holders, name)
				} else {
					holders = append([]string{name}, holders...)
				}
				break
			}
		}
	}
	if len(holders) == 0 {
		return "unknown"
	}
	return holders[0]
}

// netlinkInode is the inode of the NETLINK_ROUTE socket bound to portID.
func netlinkInode(portID uint32) string {
	data, err := os.ReadFile("/proc/net/netlink")
	if err != nil {
		return ""
	}
	port := strconv.FormatUint(uint64(portID), 10)
	// sk Eth Pid Groups Rmem Wmem Dump Locks Drops Inode
	for line := range strings.SplitSeq(string(data), "\n") {
		fields := strings.Fields(line)
		if len(fields) >= 10 && fields[1] == "0" && fields[2] == port {
			return fields[9]
		}
	}
	return ""
}

func (g *Guard) work() {
	defer g.wg.Done()
	ticker := time.NewTicker(periodic)
	defer ticker.Stop()
	var timer *time.Timer
	var timerC <-chan time.Time
	pending := request{}
	for {
		select {
		case <-g.done:
			if timer != nil {
				timer.Stop()
			}
			return
		case r := <-g.requests:
			if r.delay == 0 {
				g.check(r.reason, r.by)
				continue
			}
			if pending.reason == "" || r.by != "" {
				pending = r
			}
			if timer == nil {
				timer = time.NewTimer(r.delay)
				timerC = timer.C
			}
		case <-timerC:
			timer, timerC = nil, nil
			r := pending
			pending = request{}
			g.check(r.reason, r.by)
		case <-ticker.C:
			g.check("periodic", "")
		}
	}
}

func (g *Guard) check(reason, by string) {
	rules, routes, err := g.missing()
	if err != nil {
		g.log.Warn("tun routing check", "reason", reason, "error", err)
		return
	}
	if len(rules) == 0 && len(routes) == 0 {
		if g.broken {
			g.broken = false
			g.onChange(State{})
		}
		return
	}
	names := Names(rules, routes)
	if by == "" {
		by = "unknown"
	}
	if restoreDisabled {
		g.setBroken(names, errors.New("restoring disabled"))
		return
	}
	addErr := g.restore(rules, routes)
	stillRules, stillRoutes, err := g.missing()
	if err == nil && len(stillRules) == 0 && len(stillRoutes) == 0 {
		g.log.Warn("tun routing rules restored", "missing", strings.Join(names, ", "), "by", by, "reason", reason)
		g.flapped()
		if g.broken {
			g.broken = false
			g.onChange(State{Missing: names})
		}
		return
	}
	if err == nil {
		err = addErr
	}
	if err == nil {
		err = errors.New("still missing after restoring")
	}
	if len(stillRules)+len(stillRoutes) > 0 {
		names = Names(stillRules, stillRoutes)
	}
	g.setBroken(names, err)
}

func (g *Guard) setBroken(names []string, err error) {
	g.log.Error("tun routing broken", "missing", strings.Join(names, ", "), "error", err)
	if !g.broken {
		g.broken = true
		g.onChange(State{Broken: true, Missing: names, Err: err})
	}
}

func (g *Guard) flapped() {
	now := time.Now()
	kept := g.restores[:0]
	for _, at := range g.restores {
		if now.Sub(at) < flapWindow {
			kept = append(kept, at)
		}
	}
	g.restores = append(kept, now)
	if len(g.restores) == flapLimit+1 {
		g.log.Error("tun routing rules keep being deleted", "restores", len(g.restores), "window_s", int(flapWindow.Seconds()))
	}
}

func (g *Guard) missing() ([]Rule, []Route, error) {
	listed, err := listRules()
	if err != nil {
		return nil, nil, err
	}
	routes, err := listRoutes(g.scope.Table, g.link)
	if err != nil {
		return nil, nil, err
	}
	return Missing(g.rules, listed), Missing(g.routes, routes), nil
}

// restore adds routes first (the rules point at their table) in
// RouteRestoreOrder, then rules in RestoreOrder. EEXIST is not an error: the entry came back meanwhile.
func (g *Guard) restore(rules []Rule, routes []Route) error {
	var errs []error
	for _, r := range RouteRestoreOrder(routes) {
		if err := netlink.RouteAdd(toNetlinkRoute(r, g.scope.Table)); err != nil && !errors.Is(err, unix.EEXIST) {
			errs = append(errs, fmt.Errorf("route %s: %w", r, err))
		}
	}
	for _, r := range RestoreOrder(rules) {
		if err := netlink.RuleAdd(toNetlinkRule(r)); err != nil && !errors.Is(err, unix.EEXIST) {
			errs = append(errs, fmt.Errorf("rule %s: %w", r, err))
		}
	}
	return errors.Join(errs...)
}

// listRules dumps every rule with its action, which netlink.RuleList drops
// (without it a goto or nop rule would be added back as a table lookup).
func listRules() ([]Rule, error) {
	req := nl.NewNetlinkRequest(unix.RTM_GETRULE, unix.NLM_F_DUMP|unix.NLM_F_REQUEST)
	req.AddData(nl.NewIfInfomsg(unix.AF_UNSPEC))
	msgs, err := req.Execute(unix.NETLINK_ROUTE, unix.RTM_NEWRULE)
	if err != nil {
		return nil, fmt.Errorf("list rules: %w", err)
	}
	rules := make([]Rule, 0, len(msgs))
	for _, m := range msgs {
		rule, err := parseRule(m)
		if err != nil {
			return nil, fmt.Errorf("list rules: %w", err)
		}
		rules = append(rules, rule)
	}
	return rules, nil
}

func parseRule(b []byte) (Rule, error) {
	msg := nl.DeserializeRtMsg(b)
	attrs, err := nl.ParseRouteAttr(b[msg.Len():])
	if err != nil {
		return Rule{}, err
	}
	native := nl.NativeEndian()
	r := Rule{Family: 4, Action: msg.Type, Table: int(msg.Table), Goto: -1, Mask: -1, SuppressPrefixlen: -1,
		Invert: msg.Flags&netlink.FibRuleInvert != 0, Tos: uint(msg.Tos)}
	if msg.Family == unix.AF_INET6 {
		r.Family = 6
	}
	prefix := func(value []byte, bits uint8) netip.Prefix {
		addr, _ := netip.AddrFromSlice(value)
		return netip.PrefixFrom(addr.Unmap(), int(bits))
	}
	for _, a := range attrs {
		v := a.Value
		switch a.Attr.Type {
		case nl.FRA_PRIORITY:
			r.Priority = int(native.Uint32(v))
		case nl.FRA_TABLE:
			r.Table = int(native.Uint32(v))
		case nl.FRA_GOTO:
			r.Goto = int(native.Uint32(v))
		case nl.FRA_SRC:
			r.Src = prefix(v, msg.Src_len)
		case nl.FRA_DST:
			r.Dst = prefix(v, msg.Dst_len)
		case nl.FRA_IIFNAME:
			r.IifName = strings.TrimRight(string(v), "\x00")
		case nl.FRA_OIFNAME:
			r.OifName = strings.TrimRight(string(v), "\x00")
		case nl.FRA_FWMARK:
			r.Mark, r.MarkSet = native.Uint32(v), true
		case nl.FRA_FWMASK:
			r.Mask = int(native.Uint32(v))
		case nl.FRA_SUPPRESS_PREFIXLEN:
			if n := native.Uint32(v); n != 0xffffffff {
				r.SuppressPrefixlen = int(n)
			}
		case nl.FRA_IP_PROTO:
			r.IPProto = int(v[0])
		case nl.FRA_DPORT_RANGE:
			r.Dport = &PortRange{native.Uint16(v[0:2]), native.Uint16(v[2:4])}
		case nl.FRA_SPORT_RANGE:
			r.Sport = &PortRange{native.Uint16(v[0:2]), native.Uint16(v[2:4])}
		case nl.FRA_UID_RANGE:
			r.UIDRange = &UIDRange{native.Uint32(v[0:4]), native.Uint32(v[4:8])}
		}
	}
	if r.Action != ActionTable {
		r.Table, r.SuppressPrefixlen = -1, -1
	}
	if r.Action != ActionGoto {
		r.Goto = -1
	}
	if r.MarkSet && r.Mask == -1 {
		r.Mask = 0xffffffff
	}
	return r, nil
}

func toNetlinkRule(r Rule) *netlink.Rule {
	out := netlink.NewRule()
	out.Family = unix.AF_INET
	if r.Family == 6 {
		out.Family = unix.AF_INET6
	}
	out.Priority = r.Priority
	out.Type = r.Action
	out.Table = r.Table
	out.Goto = r.Goto
	out.Src, out.Dst = r.Src, r.Dst
	out.IifName, out.OifName = r.IifName, r.OifName
	out.Mark, out.MarkSet = r.Mark, r.MarkSet
	if r.MarkSet || r.Mask >= 0 {
		out.Mask = r.Mask
	}
	out.Invert = r.Invert
	out.SuppressPrefixlen = r.SuppressPrefixlen
	out.IPProto = r.IPProto
	out.Tos = r.Tos
	if r.Dport != nil {
		out.Dport = netlink.NewRulePortRange(r.Dport.Start, r.Dport.End)
	}
	if r.Sport != nil {
		out.Sport = netlink.NewRulePortRange(r.Sport.Start, r.Sport.End)
	}
	if r.UIDRange != nil {
		out.UIDRange = netlink.NewRuleUIDRange(r.UIDRange.Start, r.UIDRange.End)
	}
	return out
}

func listRoutes(table, link int) ([]Route, error) {
	listed, err := netlink.RouteListFiltered(netlink.FAMILY_ALL, &netlink.Route{Table: table}, netlink.RT_FILTER_TABLE)
	if err != nil {
		return nil, fmt.Errorf("list routes of table %d: %w", table, err)
	}
	var routes []Route
	for _, r := range listed {
		if r.LinkIndex != link || r.Dst == nil {
			continue
		}
		dst, ok := netip.AddrFromSlice(r.Dst.IP)
		if !ok {
			continue
		}
		bits, _ := r.Dst.Mask.Size()
		route := Route{Family: 4, Dst: netip.PrefixFrom(dst.Unmap(), bits), LinkIndex: r.LinkIndex, Priority: r.Priority}
		if r.Family == unix.AF_INET6 {
			route.Family = 6
		}
		if gw, ok := netip.AddrFromSlice(r.Gw); ok {
			route.Gw = gw.Unmap()
		}
		routes = append(routes, route)
	}
	return routes, nil
}

func toNetlinkRoute(r Route, table int) *netlink.Route {
	out := &netlink.Route{LinkIndex: r.LinkIndex, Table: table, Priority: r.Priority, Family: unix.AF_INET,
		Dst: &net.IPNet{IP: r.Dst.Addr().AsSlice(), Mask: net.CIDRMask(r.Dst.Bits(), r.Dst.Addr().BitLen())}}
	if r.Family == 6 {
		out.Family = unix.AF_INET6
	}
	if r.Gw.IsValid() {
		out.Gw = r.Gw.AsSlice()
	}
	return out
}

// routeTable is the table of a route notification.
func routeTable(b []byte) (int, bool) {
	if len(b) < unix.SizeofRtMsg {
		return 0, false
	}
	msg := nl.DeserializeRtMsg(b)
	table := int(msg.Table)
	if attrs, err := nl.ParseRouteAttr(b[msg.Len():]); err == nil {
		for _, a := range attrs {
			if a.Attr.Type == unix.RTA_TABLE && len(a.Value) >= 4 {
				table = int(nl.NativeEndian().Uint32(a.Value))
			}
		}
	}
	return table, true
}
