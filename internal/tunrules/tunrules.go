// Package tunrules keeps the Linux TUN's policy routing in place. sing-tun
// installs its rules and the routes of its table once, when the TUN starts,
// and never again; anything that deletes them (systemd-networkd drops
// foreign rules whenever a link goes down) leaves the TUN bypassed, strict
// route included, until the TUN is rebuilt. A Guard snapshots what sing-tun
// installed right after the start and puts back what goes missing.
//
// The snapshot is what was listed, not rules rebuilt from the options, so
// priorities, tables and selectors are sing-tun's by construction.
package tunrules

import (
	"fmt"
	"net/netip"
	"sort"
	"strings"
)

// Rule actions (FR_ACT_*, linux/fib_rules.h).
const (
	ActionTable       = 1
	ActionGoto        = 2
	ActionNop         = 3
	ActionBlackhole   = 6
	ActionUnreachable = 7
	ActionProhibit    = 8
)

// tableMain is RT_TABLE_MAIN.
const tableMain = 254

// Rule is a policy routing rule as the kernel lists it. Table, Goto,
// SuppressPrefixlen and Mask are -1 when absent.
type Rule struct {
	Family            int // 4 or 6
	Priority          int
	Action            uint8
	Table             int
	Goto              int
	Src, Dst          netip.Prefix
	IifName, OifName  string
	Mark              uint32
	MarkSet           bool
	Mask              int64 // a uint32, or -1; int64 so 0xffffffff fits on 32-bit
	Invert            bool
	Dport, Sport      *PortRange
	UIDRange          *UIDRange
	SuppressPrefixlen int
	IPProto           int
	Tos               uint
}

type PortRange struct{ Start, End uint16 }

type UIDRange struct{ Start, End uint32 }

// String is the rule in `ip rule` terms, prefixed by priority and family
// ("9093/v4 not dport 53 lookup 254 suppress_prefixlength 0"). It is also
// the rule's identity: two rules with the same string are the same rule.
func (r Rule) String() string {
	var b strings.Builder
	fmt.Fprintf(&b, "%d/v%d", r.Priority, r.Family)
	if r.Invert {
		b.WriteString(" not")
	}
	if r.Src.IsValid() {
		fmt.Fprintf(&b, " from %s", r.Src)
	}
	if r.Dst.IsValid() {
		fmt.Fprintf(&b, " to %s", r.Dst)
	}
	if r.Tos != 0 {
		fmt.Fprintf(&b, " tos %d", r.Tos)
	}
	if r.MarkSet || r.Mask >= 0 {
		fmt.Fprintf(&b, " fwmark %#x/%#x", r.Mark, r.Mask)
	}
	if r.IifName != "" {
		fmt.Fprintf(&b, " iif %s", r.IifName)
	}
	if r.OifName != "" {
		fmt.Fprintf(&b, " oif %s", r.OifName)
	}
	if r.IPProto != 0 {
		fmt.Fprintf(&b, " ipproto %d", r.IPProto)
	}
	if r.Sport != nil {
		fmt.Fprintf(&b, " sport %d-%d", r.Sport.Start, r.Sport.End)
	}
	if r.Dport != nil {
		fmt.Fprintf(&b, " dport %d-%d", r.Dport.Start, r.Dport.End)
	}
	if r.UIDRange != nil {
		fmt.Fprintf(&b, " uidrange %d-%d", r.UIDRange.Start, r.UIDRange.End)
	}
	switch r.Action {
	case ActionTable:
		fmt.Fprintf(&b, " lookup %d", r.Table)
		if r.SuppressPrefixlen >= 0 {
			fmt.Fprintf(&b, " suppress_prefixlength %d", r.SuppressPrefixlen)
		}
	case ActionGoto:
		fmt.Fprintf(&b, " goto %d", r.Goto)
	case ActionNop:
		b.WriteString(" nop")
	case ActionBlackhole:
		b.WriteString(" blackhole")
	case ActionUnreachable:
		b.WriteString(" unreachable")
	case ActionProhibit:
		b.WriteString(" prohibit")
	default:
		fmt.Fprintf(&b, " action %d", r.Action)
	}
	return b.String()
}

// selectorless: the rule matches every packet.
func (r Rule) selectorless() bool {
	return !r.Invert && !r.Src.IsValid() && !r.Dst.IsValid() && r.Tos == 0 && !r.MarkSet && r.Mask < 0 &&
		r.IifName == "" && r.OifName == "" && r.IPProto == 0 && r.Sport == nil && r.Dport == nil && r.UIDRange == nil
}

// Scope is the namespace sing-tun was given: the TUN interface, its table
// and its rule priorities [RuleStart, RuleEnd] (sing-tun's cleanup claims
// exactly that range).
type Scope struct {
	Interface          string
	Table              int
	RuleStart, RuleEnd int
}

func (s Scope) inRange(priority int) bool { return priority >= s.RuleStart && priority <= s.RuleEnd }

// Owned splits the rules listed in the scope's priority range into the ones
// sing-tun installs and the rest. A rule is sing-tun's when it looks up the
// scope's table, jumps within the range, names the TUN interface, or is one
// of sing-tun's two selector-free kinds: the nop the range's gotos land on
// and the unreachable rules of strict route; plus its DNS rule
// ("not dport 53 lookup main suppress_prefixlength 0"). Anything else in the
// range was put there by another program after sing-tun's start and is not
// ours to restore.
func (s Scope) Owned(rules []Rule) (owned, foreign []Rule) {
	for _, r := range rules {
		if !s.inRange(r.Priority) {
			continue
		}
		if s.owns(r) {
			owned = append(owned, r)
		} else {
			foreign = append(foreign, r)
		}
	}
	return owned, foreign
}

func (s Scope) owns(r Rule) bool {
	switch {
	case r.Action == ActionTable && r.Table == s.Table:
		return true
	case r.Action == ActionGoto && s.inRange(r.Goto):
		return true
	case s.Interface != "" && (r.IifName == s.Interface || r.OifName == s.Interface):
		return true
	case (r.Action == ActionNop || r.Action == ActionUnreachable) && r.selectorless():
		return true
	}
	dns := r
	dns.Invert, dns.Dport = false, nil
	return r.Invert && r.Dport != nil && *r.Dport == (PortRange{53, 53}) &&
		r.Action == ActionTable && r.Table == tableMain && r.SuppressPrefixlen == 0 && dns.selectorless()
}

// Route is a route of the scope's table.
type Route struct {
	Family    int // 4 or 6
	Dst       netip.Prefix
	Gw        netip.Addr
	LinkIndex int
	Priority  int
}

func (r Route) String() string {
	s := r.Dst.String()
	if r.Gw.IsValid() {
		s += " via " + r.Gw.String()
	}
	return fmt.Sprintf("%s dev#%d metric %d", s, r.LinkIndex, r.Priority)
}

// Missing returns the entries of want not in have (counting duplicates),
// in want's order.
func Missing[T fmt.Stringer](want, have []T) []T {
	count := make(map[string]int, len(have))
	for _, item := range have {
		count[item.String()]++
	}
	var missing []T
	for _, item := range want {
		key := item.String()
		if count[key] > 0 {
			count[key]--
			continue
		}
		missing = append(missing, item)
	}
	return missing
}

// RestoreOrder sorts rules for adding back: highest priority number first,
// so a goto's target (the range's nop) exists before the goto and is never
// shown as unresolved.
func RestoreOrder(rules []Rule) []Rule {
	out := append([]Rule(nil), rules...)
	sort.SliceStable(out, func(i, j int) bool { return out[i].Priority > out[j].Priority })
	return out
}

// RouteRestoreOrder puts last the routes whose own prefix covers their
// gateway (fc00::/7 via the TUN's fde2:…::2). The kernel checks a new
// route's gateway in the route's table first: once such a route is back,
// every gateway route added after it resolves its gateway through it, not
// on-link, and is refused (EHOSTUNREACH).
func RouteRestoreOrder(routes []Route) []Route {
	out := append([]Route(nil), routes...)
	sort.SliceStable(out, func(i, j int) bool { return !out[i].coversGateway() && out[j].coversGateway() })
	return out
}

func (r Route) coversGateway() bool { return r.Gw.IsValid() && r.Dst.Contains(r.Gw) }

// Names is the rules' and routes' strings, for logs and events.
func Names(rules []Rule, routes []Route) []string {
	names := make([]string, 0, len(rules)+len(routes))
	for _, r := range rules {
		names = append(names, r.String())
	}
	for _, r := range routes {
		names = append(names, "route "+r.String())
	}
	return names
}

// State is the guard's verdict, reported on every change.
type State struct {
	// Broken: rules or routes are missing and could not be put back. The
	// TUN is then (partly) bypassed; the host should restart the core.
	Broken bool
	// Missing lists what is missing (Broken) or was missing (restored).
	Missing []string
	Err     error
}
