package config

import (
	C "github.com/sagernet/sing-box/constant"
	"net/netip"
	"slices"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/proxyinbound"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/option"
	tunpkg "github.com/sagernet/sing-tun"
)

func proxyEndpoint(nodeID string) localproxy.Endpoint {
	return localproxy.Endpoint{NodeID: nodeID, Listen: "127.0.0.1", Port: 7890, Username: localproxy.FormatUsername("u8f2k", nodeID), Password: "shared-secret"}
}

func TestEachLocalProxyUserRoutesToItsNode(t *testing.T) {
	a := node(profile.ProtocolShadowsocks)
	a.ID = "a"
	a.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	b := a
	b.ID = "b-with-dash"
	b.Ingresses = append([]profile.Ingress(nil), a.Ingresses...)
	b.Ingresses[0].EndpointKey = "b-9001"
	p := &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "r", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{a, b}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "a"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
	proxies := []localproxy.Endpoint{proxyEndpoint("a"), proxyEndpoint("b-with-dash")}
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, proxies, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Inbounds) != 1 {
		t.Fatalf("inbounds: %d", len(got.Options.Inbounds))
	}
	in := got.Options.Inbounds[0]
	options := in.Options.(*proxyinbound.Options)
	if in.Type != proxyinbound.Type || in.Tag != LocalProxyInboundTag || options.ListenPort != 7890 || options.Listen.Build(netip.Addr{}).String() != "127.0.0.1" || len(options.Users) != 3 {
		t.Fatalf("inbound: %#v", in)
	}
	if len(got.Options.Route.Rules) != 3 {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
	for i, proxy := range proxies {
		rule := got.Options.Route.Rules[i].DefaultOptions
		if options.Users[i].Username != proxy.Username || options.Users[i].Password != "shared-secret" {
			t.Fatalf("user %d: %#v", i, options.Users[i])
		}
		if len(rule.Inbound) != 1 || rule.Inbound[0] != LocalProxyInboundTag || len(rule.AuthUser) != 1 || rule.AuthUser[0] != proxy.Username ||
			rule.RouteOptions.Outbound != got.NodeTags[proxy.NodeID] {
			t.Fatalf("route %d: %#v", i, rule)
		}
	}
	// The routed user (bare prefix) has no rule of its own; the catch-all
	// rejects every other local proxy user, so it alone falls through to the
	// profile rules.
	if routed := options.Users[2]; routed.Username != "u8f2k" || routed.Password != "shared-secret" {
		t.Fatalf("routed user: %#v", routed)
	}
	assertLocalProxyCatchAll(t, got.Options.Route.Rules[2], "u8f2k")
}

// assertLocalProxyCatchAll checks the rule rejecting local proxy traffic
// except the routed user's.
func assertLocalProxyCatchAll(t *testing.T, rule option.Rule, routed string) {
	t.Helper()
	logical := rule.LogicalOptions
	if rule.Type != C.RuleTypeLogical || logical.Mode != C.LogicalTypeAnd || logical.Action != C.RuleActionTypeReject || len(logical.Rules) != 2 {
		t.Fatalf("catch-all: %#v", rule)
	}
	inbound, notRouted := logical.Rules[0].DefaultOptions, logical.Rules[1].DefaultOptions
	if len(inbound.Inbound) != 1 || inbound.Inbound[0] != LocalProxyInboundTag || inbound.Invert ||
		len(notRouted.AuthUser) != 1 || notRouted.AuthUser[0] != routed || !notRouted.Invert {
		t.Fatalf("catch-all conditions: %#v", logical.Rules)
	}
}

func TestLocalProxyRejectsInconsistentEndpoints(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	m := n
	m.ID = "other"
	m.Ingresses = append([]profile.Ingress(nil), n.Ingresses...)
	m.Ingresses[0].EndpointKey = "other-9001"
	p := base(n)
	p.Nodes = append(p.Nodes, m)
	for name, mutate := range map[string]func(*localproxy.Endpoint){
		"port":     func(e *localproxy.Endpoint) { e.Port++ },
		"password": func(e *localproxy.Endpoint) { e.Password = "different" },
		"username": func(e *localproxy.Endpoint) { e.Username = localproxy.FormatUsername("zzzzz", e.NodeID) },
		"listen":   func(e *localproxy.Endpoint) { e.Listen = "0.0.0.0" },
	} {
		second := proxyEndpoint(m.ID)
		mutate(&second)
		if _, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, []localproxy.Endpoint{proxyEndpoint(n.ID), second}, time.Now()); err == nil {
			t.Fatalf("%s mismatch accepted", name)
		}
	}
}

func TestPlatformCapabilitiesStayOutsideProfile(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	p := base(n)
	proxy := proxyEndpoint(n.ID)
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{Platform: "macos", TUN: profile.TUNCapabilities{Enabled: true, Stack: "mixed"}}, []localproxy.Endpoint{proxy}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Inbounds) != 2 {
		t.Fatalf("inbounds: %d", len(got.Options.Inbounds))
	}
	shared := got.Options.Inbounds[0].Options.(*proxyinbound.Options)
	tun := got.Options.Inbounds[1].Options.(*option.TunInboundOptions)
	if len(shared.Users) != 2 || !tun.AutoRoute || tun.Stack != "mixed" {
		t.Fatalf("shared=%#v tun=%#v", shared, tun)
	}
}

func TestDesktopTUNUsesOwnIPRoute2Namespace(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	for _, platform := range []string{"linux", "macos", "windows"} {
		got, err := Build(base(n), profile.PlatformCapabilities{Platform: platform, TUN: profile.TUNCapabilities{Enabled: true}}, time.Now())
		if err != nil {
			t.Fatal(err)
		}
		tun := got.Options.Inbounds[len(got.Options.Inbounds)-1].Options.(*option.TunInboundOptions)
		if tun.IPRoute2TableIndex != 2091 || tun.IPRoute2RuleIndex != 9091 {
			t.Fatalf("%s: table=%d rule=%d", platform, tun.IPRoute2TableIndex, tun.IPRoute2RuleIndex)
		}
		// sing-tun owns [rule, rule+10]; it must not overlap its defaults.
		if tun.IPRoute2TableIndex == tunpkg.DefaultIPRoute2TableIndex || tun.IPRoute2RuleIndex <= tunpkg.DefaultIPRoute2RuleIndex+10 {
			t.Fatalf("%s: collides with sing-tun defaults", platform)
		}
	}
}

// Desktop TUN carries an IPv6 address so IPv6 (and DNS to IPv6 resolvers)
// is routed into the tunnel instead of around it; every ingress IP, IPv4 or
// IPv6, stays excluded. Mobile hosts build the tunnel and stay IPv4-only.
func TestDesktopTUNRoutesIPv6AndExcludesIPv6Ingress(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses = append(n.Ingresses, ingress(profile.ProtocolShadowsocks, profile.IngressRoleBackup, "v6.example.com", "2606:4700:4700::1111"))
	n.Ingresses[1].EndpointKey = "v6-backup"
	n.Ingresses[1].ReplicaOrdinal = n.Ingresses[0].ReplicaOrdinal + 1
	inet4, inet6 := netip.MustParsePrefix("10.60.159.89/30"), netip.MustParsePrefix("fde2:ec40:9312:c7fd::1/126")
	for _, platform := range []string{"linux", "macos", "windows"} {
		got, err := Build(base(n), profile.PlatformCapabilities{Platform: platform, TUN: profile.TUNCapabilities{Enabled: true}}, time.Now())
		if err != nil {
			t.Fatal(err)
		}
		tun := got.Options.Inbounds[len(got.Options.Inbounds)-1].Options.(*option.TunInboundOptions)
		if len(tun.Address) != 2 || tun.Address[0] != inet4 || tun.Address[1] != inet6 {
			t.Fatalf("%s: address %v", platform, tun.Address)
		}
		if !tun.AutoRoute || !tun.StrictRoute {
			t.Fatalf("%s: auto_route=%v strict_route=%v", platform, tun.AutoRoute, tun.StrictRoute)
		}
		want := mustPrefixes("8.8.8.8/32", "2606:4700:4700::1111/128", "224.0.0.0/4", "255.255.255.255/32", "169.254.0.0/16", "fe80::/10", "ff00::/8")
		if !slices.Equal(tun.RouteExcludeAddress, want) {
			t.Fatalf("%s: route_exclude_address %v", platform, tun.RouteExcludeAddress)
		}
	}
	for _, platform := range []string{"ios", "android"} {
		got, err := Build(base(n), profile.PlatformCapabilities{Platform: platform, TUN: profile.TUNCapabilities{Enabled: true}}, time.Now())
		if err != nil {
			t.Fatal(err)
		}
		tun := got.Options.Inbounds[len(got.Options.Inbounds)-1].Options.(*option.TunInboundOptions)
		if len(tun.Address) != 1 || tun.Address[0] != inet4 || tun.AutoRoute || len(tun.RouteExcludeAddress) != 0 {
			t.Fatalf("%s: tun %#v", platform, tun)
		}
	}
}

// A host with IPv6 disabled cannot give the TUN an IPv6 address (sing-tun
// fails the whole start), so the desktop TUN stays IPv4-only there and no
// IPv6 ingress prefix is excluded from routes that are never installed.
func TestDesktopTUNWithoutHostIPv6IsIPv4Only(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses = append(n.Ingresses, ingress(profile.ProtocolShadowsocks, profile.IngressRoleBackup, "v6.example.com", "2606:4700:4700::1111"))
	n.Ingresses[1].EndpointKey = "v6-backup"
	n.Ingresses[1].ReplicaOrdinal = n.Ingresses[0].ReplicaOrdinal + 1
	for _, platform := range []string{"linux", "macos", "windows"} {
		got, err := BuildWithOptions(base(n), profile.PlatformCapabilities{Platform: platform, TUN: profile.TUNCapabilities{Enabled: true}}, BuildOptions{DisableTUNIPv6: true}, time.Now())
		if err != nil {
			t.Fatal(err)
		}
		tun := got.Options.Inbounds[len(got.Options.Inbounds)-1].Options.(*option.TunInboundOptions)
		if len(tun.Address) != 1 || tun.Address[0] != netip.MustParsePrefix("10.60.159.89/30") {
			t.Fatalf("%s: address %v", platform, tun.Address)
		}
		if !tun.AutoRoute || !tun.StrictRoute {
			t.Fatalf("%s: auto_route=%v strict_route=%v", platform, tun.AutoRoute, tun.StrictRoute)
		}
		if want := mustPrefixes("8.8.8.8/32", "224.0.0.0/4", "255.255.255.255/32", "169.254.0.0/16"); !slices.Equal(tun.RouteExcludeAddress, want) {
			t.Fatalf("%s: route_exclude_address %v", platform, tun.RouteExcludeAddress)
		}
	}
}

func TestPrivateBypassRuleFollowsPerNodeRules(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	p := base(n)
	p.Routing.Rules = []profile.RoutingRule{{
		ID:     "private-direct",
		Match:  profile.RoutingMatch{IPIsPrivate: true},
		Action: profile.RoutingAction{Type: "direct"},
	}}
	proxy := proxyEndpoint(n.ID)
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, []localproxy.Endpoint{proxy}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Route.Rules) != 3 {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
	privateRule := got.Options.Route.Rules[2].DefaultOptions
	if got.Options.Route.Rules[0].DefaultOptions.Inbound == nil ||
		len(privateRule.IPCIDR) != len(profile.PrivatePrefixes) ||
		privateRule.RouteOptions.Outbound != "direct" {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
}

func TestRuleMappingAndFixedPriority(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	p := base(n)
	p.Routing.Rules = []profile.RoutingRule{{
		ID: "ordered",
		Match: profile.RoutingMatch{
			Domains:        []string{"例子.测试."},
			DomainSuffixes: []string{"Example.COM"},
			IPCIDRs:        []string{"2001:4860:0:1::1/32"},
			Protocols:      []string{"tcp"},
			Ports:          []uint16{443},
			PortRanges:     []string{"8000-9000"},
		},
		Action: profile.RoutingAction{Type: "reject"},
	}}
	proxy := proxyEndpoint(n.ID)
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, []localproxy.Endpoint{proxy}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Route.Rules) != 3 {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
	if got.Options.Route.Rules[0].DefaultOptions.Inbound == nil {
		t.Fatal("local proxy routes must precede profile rules")
	}
	assertLocalProxyCatchAll(t, got.Options.Route.Rules[1], "u8f2k")
	rule := got.Options.Route.Rules[2].DefaultOptions
	if len(rule.Domain) != 2 || rule.Domain[0] != "xn--fsqu00a.xn--0zwm56d" ||
		len(rule.DomainSuffix) != 1 || rule.DomainSuffix[0] != ".example.com" ||
		len(rule.IPCIDR) != 1 || rule.IPCIDR[0] != "2001:4860::/32" ||
		len(rule.Network) != 1 || rule.Network[0] != "tcp" ||
		len(rule.Port) != 1 || rule.Port[0] != 443 ||
		len(rule.PortRange) != 1 || rule.PortRange[0] != "8000:9000" ||
		rule.Action != "reject" {
		t.Fatalf("mapped rule: %#v", rule)
	}
}

func mustPrefixes(values ...string) []netip.Prefix {
	out := make([]netip.Prefix, len(values))
	for i, v := range values {
		out[i] = netip.MustParsePrefix(v)
	}
	return out
}
