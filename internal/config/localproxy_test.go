package config

import (
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/systemproxy"
	"github.com/sagernet/sing-box/option"
	"testing"
	"time"
)

func TestSystemProxyRejectsExternalAndInvalidListen(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	for _, listen := range []string{"", "0.0.0.0", "192.168.1.20", "localhost", "example.com"} {
		endpoint := systemproxy.Endpoint{Listen: listen, Port: 10001}
		platform := profile.PlatformCapabilities{SystemProxy: profile.SystemProxyCapabilities{Enabled: true, Listen: listen}}
		if _, err := BuildWithProxyEndpoints(base(n), platform, nil, &endpoint, time.Now()); err == nil {
			t.Fatalf("accepted system proxy listen %q", listen)
		}
	}
}

func TestSystemProxyRoutesThroughInterruptingSelector(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	endpoint := systemproxy.Endpoint{Listen: "127.0.0.1", Port: 10001}
	platform := profile.PlatformCapabilities{SystemProxy: profile.SystemProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}}
	got, err := BuildWithProxyEndpoints(base(n), platform, nil, &endpoint, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	selector := got.Options.Outbounds[0].Options.(*option.SelectorOutboundOptions)
	if !selector.InterruptExistConnections || got.Options.Route == nil || got.Options.Route.Final != selectedOutboundTag {
		t.Fatalf("selector or route does not enforce selected outbound: %#v %#v", selector, got.Options.Route)
	}
	system := got.Options.Inbounds[0].Options.(*option.HTTPMixedInboundOptions)
	if len(system.Users) != 0 || system.ListenPort != endpoint.Port {
		t.Fatalf("system proxy unexpectedly authenticated or moved: %#v", system)
	}
}

func TestEachLocalProxyRoutesToItsNode(t *testing.T) {
	a := node(profile.ProtocolShadowsocks)
	a.ID = "a"
	a.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	b := a
	b.ID = "b"
	p := &profile.Profile{SchemaVersion: 1, Revision: "r", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{a, b}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "a"}}
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
	endpoint := systemproxy.Endpoint{Listen: "127.0.0.1", Port: 10002}
	got, err := BuildWithProxyEndpoints(p, profile.PlatformCapabilities{Platform: "macos", TUN: profile.TUNCapabilities{Enabled: true, Stack: "mixed"}, SystemProxy: profile.SystemProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}}, []localproxy.Endpoint{proxy}, &endpoint, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Inbounds) != 3 {
		t.Fatalf("inbounds: %d", len(got.Options.Inbounds))
	}
	perNode := got.Options.Inbounds[0].Options.(*option.HTTPMixedInboundOptions)
	system := got.Options.Inbounds[1].Options.(*option.HTTPMixedInboundOptions)
	tun := got.Options.Inbounds[2].Options.(*option.TunInboundOptions)
	if perNode.SetSystemProxy || len(perNode.Users) != 1 || system.SetSystemProxy || len(system.Users) != 0 || !tun.AutoRoute || tun.Stack != "mixed" {
		t.Fatalf("perNode=%#v system=%#v tun=%#v", perNode, system, tun)
	}
}

func TestPrivateBypassRuleFollowsPerNodeRules(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	p := base(n)
	p.Routing.BypassPrivate = true
	proxy := localproxy.Endpoint{NodeID: n.ID, Listen: "127.0.0.1", Port: 10001, Username: "u", Password: "p"}
	got, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{}, []localproxy.Endpoint{proxy}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Options.Route.Rules) != 2 || got.Options.Route.Rules[0].DefaultOptions.Inbound == nil || !got.Options.Route.Rules[1].DefaultOptions.IPIsPrivate || got.Options.Route.Rules[1].DefaultOptions.RouteOptions.Outbound != "direct" {
		t.Fatalf("rules: %#v", got.Options.Route.Rules)
	}
}
