package tunrules

import (
	"net/netip"
	"reflect"
	"testing"
)

var scope = Scope{Interface: "tun0", Table: 2091, RuleStart: 9091, RuleEnd: 9101}

func rule(priority int, action uint8, set func(*Rule)) Rule {
	r := Rule{Family: 4, Priority: priority, Action: action, Table: -1, Goto: -1, Mask: -1, SuppressPrefixlen: -1}
	if set != nil {
		set(&r)
	}
	return r
}

// singTunRules is what sing-tun 0.8.9 installed on a Linux host (IPv4 part
// of `ip rule` from Desktop's test, with strict route).
func singTunRules() []Rule {
	return []Rule{
		rule(9091, ActionTable, func(r *Rule) { r.Dst = netip.MustParsePrefix("10.60.159.88/30"); r.Table = 2091 }),
		rule(9092, ActionTable, func(r *Rule) { r.Table = 2091; r.SuppressPrefixlen = 0 }),
		rule(9093, ActionTable, func(r *Rule) {
			r.Invert = true
			r.Dport = &PortRange{53, 53}
			r.Table = tableMain
			r.SuppressPrefixlen = 0
		}),
		rule(9093, ActionGoto, func(r *Rule) { r.IifName = "tun0"; r.Goto = 9101 }),
		rule(9094, ActionTable, func(r *Rule) { r.Invert = true; r.IifName = "lo"; r.Table = 2091 }),
		rule(9094, ActionTable, func(r *Rule) {
			r.Src = netip.MustParsePrefix("0.0.0.0/32")
			r.IifName = "lo"
			r.Table = 2091
		}),
		rule(9101, ActionNop, nil),
		rule(9095, ActionUnreachable, func(r *Rule) { r.Family = 6 }),
	}
}

func TestOwnedKeepsEverySingTunRule(t *testing.T) {
	listed := append([]Rule{
		rule(0, ActionTable, func(r *Rule) { r.Table = 255 }),
		rule(32766, ActionTable, func(r *Rule) { r.Table = tableMain }),
	}, singTunRules()...)
	owned, foreign := scope.Owned(listed)
	if !reflect.DeepEqual(owned, singTunRules()) || len(foreign) != 0 {
		t.Fatalf("owned:\n%v\nforeign:\n%v", owned, foreign)
	}
}

func TestOwnedLeavesOtherProgramsRules(t *testing.T) {
	others := []Rule{
		rule(9095, ActionTable, func(r *Rule) { r.Table = 100 }),
		rule(9096, ActionTable, func(r *Rule) { r.Mark, r.MarkSet, r.Mask = 1, true, 0xffffffff; r.Table = tableMain }),
		rule(9097, ActionGoto, func(r *Rule) { r.Goto = 32766 }),
		rule(9098, ActionNop, func(r *Rule) { r.IifName = "eth0" }),
		rule(9099, ActionTable, func(r *Rule) { r.Invert = true; r.Dport = &PortRange{53, 53}; r.Table = 100; r.SuppressPrefixlen = 0 }),
	}
	owned, foreign := scope.Owned(append(singTunRules(), others...))
	if !reflect.DeepEqual(owned, singTunRules()) || !reflect.DeepEqual(foreign, others) {
		t.Fatalf("owned:\n%v\nforeign:\n%v", owned, foreign)
	}
}

func TestMissingCountsDuplicates(t *testing.T) {
	want := singTunRules()
	// What networkd left in Desktop's test: only the goto it failed to drop.
	have := []Rule{want[3]}
	missing := Missing(want, have)
	if len(missing) != len(want)-1 {
		t.Fatalf("missing %d of %d: %v", len(missing), len(want), missing)
	}
	for _, r := range missing {
		if r.String() == want[3].String() {
			t.Fatalf("%s is present but reported missing", r)
		}
	}
	if got := Missing(want, append(want, want[0])); len(got) != 0 {
		t.Fatalf("nothing is missing, got %v", got)
	}
	twice := append(append([]Rule(nil), want...), want[0])
	if got := Missing(twice, want); len(got) != 1 {
		t.Fatalf("one of two identical rules is missing, got %v", got)
	}
}

func TestRestoreOrderPutsGotoTargetsFirst(t *testing.T) {
	ordered := RestoreOrder(singTunRules())
	if ordered[0].Action != ActionNop || ordered[0].Priority != 9101 {
		t.Fatalf("first restored: %s", ordered[0])
	}
	for i := 1; i < len(ordered); i++ {
		if ordered[i].Priority > ordered[i-1].Priority {
			t.Fatalf("order: %v", ordered)
		}
	}
}

func TestRuleString(t *testing.T) {
	for _, tc := range []struct {
		rule Rule
		want string
	}{
		{singTunRules()[2], "9093/v4 not dport 53-53 lookup 254 suppress_prefixlength 0"},
		{singTunRules()[3], "9093/v4 iif tun0 goto 9101"},
		{singTunRules()[6], "9101/v4 nop"},
		{rule(9091, ActionGoto, func(r *Rule) { r.UIDRange = &UIDRange{1000, 1000}; r.Goto = 9101 }), "9091/v4 uidrange 1000-1000 goto 9101"},
	} {
		if got := tc.rule.String(); got != tc.want {
			t.Errorf("got %q, want %q", got, tc.want)
		}
	}
}

func TestRouteRestoreOrderPutsGatewayCoveringRoutesLast(t *testing.T) {
	gw := netip.MustParseAddr("fde2:ec40:9312:c7fd::2")
	route := func(prefix string) Route {
		return Route{Family: 6, Dst: netip.MustParsePrefix(prefix), Gw: gw, LinkIndex: 2, Priority: 1024}
	}
	// Table 2091's IPv6 routes as listed (sorted by prefix).
	listed := []Route{route("::/1"), route("8000::/2"), route("fc00::/7"), route("fe00::/9"), route("fec0::/10")}
	ordered := RouteRestoreOrder(listed)
	want := []Route{route("::/1"), route("8000::/2"), route("fe00::/9"), route("fec0::/10"), route("fc00::/7")}
	if !reflect.DeepEqual(ordered, want) {
		t.Fatalf("order:\n%v\nwant\n%v", ordered, want)
	}
}
