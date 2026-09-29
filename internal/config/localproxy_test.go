package config

import (
	"net/netip"

	"github.com/peakpassvpn/ppvpn-core/internal/proxyinbound"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/option"
	"testing"
	"time"
)

func proxyEndpoint(nodeID string) localproxy.Endpoint {
	return localproxy.Endpoint{NodeID: nodeID, Listen: "127.0.0.1", Port: 7890, Username: localproxy.FormatUsername("u8f2k", nodeID), Password: "shared-secret"}
}

func TestEachLocalProxyUserRoutesToItsNode(t *testing.T) {
	a := node(profile.ProtocolShadowsocks)
	a.ID = "a"
	a.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
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
	if in.Type != proxyinbound.Type || in.Tag != LocalProxyInboundTag || options.ListenPort != 7890 || options.Listen.Build(netip.Addr{}).String() != "127.0.0.1" || len(options.Users) != 2 {
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
	catchAll := got.Options.Route.Rules[2].DefaultOptions
	if len(catchAll.Inbound) != 1 || len(catchAll.AuthUser) != 0 || catchAll.Action != "reject" {
		t.Fatalf("catch-all: %#v", catchAll)
	}
}

func TestLocalProxyRejectsInconsistentEndpoints(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
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
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
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
	if len(shared.Users) != 1 || !tun.AutoRoute || tun.Stack != "mixed" {
		t.Fatalf("shared=%#v tun=%#v", shared, tun)
	}
}

func TestPrivateBypassRuleFollowsPerNodeRules(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
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
		len(privateRule.IPCIDR) != 4 ||
		privateRule.RouteOptions.Outbound != "direct" {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
}

func TestRuleMappingAndFixedPriority(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
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
	if got.Options.Route.Rules[0].DefaultOptions.Inbound == nil || got.Options.Route.Rules[1].DefaultOptions.Inbound == nil {
		t.Fatal("local proxy routes must precede profile rules")
	}
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
