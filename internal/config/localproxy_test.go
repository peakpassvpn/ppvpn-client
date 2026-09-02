package config

import (
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/option"
	"testing"
	"time"
)

func TestEachLocalProxyRoutesToItsNode(t *testing.T) {
	a := node(profile.ProtocolShadowsocks)
	a.ID = "a"
	a.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	b := a
	b.ID = "b"
	p := &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "r", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{a, b}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "a"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
	proxies := []localproxy.Endpoint{{NodeID: "a", Listen: "127.0.0.1", Port: 10001, Username: "ua", Password: "pa"}, {NodeID: "b", Listen: "127.0.0.1", Port: 10002, Username: "ub", Password: "pb"}}
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, proxies, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Inbounds) != 2 || len(got.Options.Route.Rules) != 2 {
		t.Fatal("missing per-node routes")
	}
	for i, rule := range got.Options.Route.Rules {
		out := rule.DefaultOptions.RouteOptions.Outbound
		if out != got.NodeTags[proxies[i].NodeID] {
			t.Fatalf("route %d: %s", i, out)
		}
		in := got.Options.Inbounds[i].Options.(*option.HTTPMixedInboundOptions)
		if in.ListenPort != proxies[i].Port || len(in.Users) != 1 {
			t.Fatalf("inbound %d: %#v", i, in)
		}
	}
}

func TestPlatformCapabilitiesStayOutsideProfile(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	p := base(n)
	proxy := localproxy.Endpoint{NodeID: n.ID, Listen: "127.0.0.1", Port: 10001, Username: "u", Password: "p"}
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{Platform: "macos", TUN: profile.TUNCapabilities{Enabled: true, Stack: "mixed"}}, []localproxy.Endpoint{proxy}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Inbounds) != 2 {
		t.Fatalf("inbounds: %d", len(got.Options.Inbounds))
	}
	perNode := got.Options.Inbounds[0].Options.(*option.HTTPMixedInboundOptions)
	tun := got.Options.Inbounds[1].Options.(*option.TunInboundOptions)
	if perNode.SetSystemProxy || len(perNode.Users) != 1 || !tun.AutoRoute || tun.Stack != "mixed" {
		t.Fatalf("perNode=%#v tun=%#v", perNode, tun)
	}
}

func TestPrivateBypassRuleFollowsPerNodeRules(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	p := base(n)
	p.Routing.Rules = []profile.RoutingRule{{
		ID:     "private-direct",
		Match:  profile.RoutingMatch{IPIsPrivate: true},
		Action: profile.RoutingAction{Type: "direct"},
	}}
	proxy := localproxy.Endpoint{NodeID: n.ID, Listen: "127.0.0.1", Port: 10001, Username: "u", Password: "p"}
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, []localproxy.Endpoint{proxy}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Route.Rules) != 2 {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
	privateRule := got.Options.Route.Rules[1].DefaultOptions
	if got.Options.Route.Rules[0].DefaultOptions.Inbound == nil ||
		len(privateRule.IPCIDR) != 4 ||
		privateRule.RouteOptions.Outbound != "direct" {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
}

func TestV2RuleMappingAndFixedPriority(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
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
	proxy := localproxy.Endpoint{NodeID: n.ID, Listen: "127.0.0.1", Port: 10001, Username: "u", Password: "p"}
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, []localproxy.Endpoint{proxy}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Route.Rules) != 2 {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
	if got.Options.Route.Rules[0].DefaultOptions.Inbound == nil {
		t.Fatal("per-node route must precede profile rules")
	}
	rule := got.Options.Route.Rules[1].DefaultOptions
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
