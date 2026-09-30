package config

import (
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/option"
)

func base(n profile.Node) *profile.Profile {
	return &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion,
		Revision:      "rev",
		ExpiresAt:     time.Now().Add(time.Hour),
		Nodes:         []profile.Node{n},
		Selection:     profile.Selection{Mode: "manual", DefaultNodeID: n.ID},
		Routing:       profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
}
func node(protocol profile.Protocol) profile.Node {
	return profile.Node{ID: "stable", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{ingress(protocol, profile.IngressRolePrimary, "edge.example.com", "8.8.8.8")}}
}

// ingress returns an ingress with protocol-appropriate TLS but no
// credentials; tests fill in credentials explicitly.
func ingress(protocol profile.Protocol, role profile.IngressRole, domain, ip string) profile.Ingress {
	in := profile.Ingress{Role: role, EndpointKey: domain, Protocol: protocol, Endpoint: profile.Endpoint{Domain: domain, IP: ip, Port: 443}, Capabilities: profile.Capabilities{TCP: true, UDP: true}}
	switch protocol {
	case profile.ProtocolShadowsocks:
		in.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	case profile.ProtocolVLESS:
		in.Credentials.VLESS = &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000002"}
		in.TLS = &profile.TLS{ServerName: domain, Reality: &profile.Reality{PublicKey: "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA", ShortID: "02"}}
	case profile.ProtocolAnyTLS:
		in.Credentials.AnyTLS = &profile.AnyTLSCredentials{Password: "backup-password"}
		in.TLS = &profile.TLS{ServerName: domain}
	}
	return in
}
// A node outbound dials the ingress IP when the profile gives one, so
// reaching a node never needs DNS (in TUN mode the lookup can loop into the
// core's own tunnel DNS); the TLS/REALITY server name stays the domain.
// Without an IP it dials the domain.
func TestNodeOutboundDialsIngressIP(t *testing.T) {
	cases := []struct {
		name, ip, want string
	}{
		{"ip", "8.8.8.8", "8.8.8.8"},
		{"mapped ipv4", "::ffff:8.8.4.4", "8.8.4.4"},
		{"ipv6", "2606:4700:4700::1111", "2606:4700:4700::1111"},
		{"no ip", "", "edge.example.com"},
	}
	for _, tc := range cases {
		n := node(profile.ProtocolVLESS)
		n.Ingresses[0].Endpoint.IP = tc.ip
		n.Ingresses[0].Credentials.VLESS = &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001"}
		got, err := Build(base(n), profile.PlatformCapabilities{}, time.Now())
		if err != nil {
			t.Fatalf("%s: %v", tc.name, err)
		}
		var o *option.VLESSOutboundOptions
		for _, out := range got.Options.Outbounds {
			if v, ok := out.Options.(*option.VLESSOutboundOptions); ok {
				o = v
			}
		}
		if o == nil || o.Server != tc.want || o.TLS == nil || o.TLS.ServerName != "edge.example.com" {
			t.Fatalf("%s: %#v", tc.name, o)
		}
	}
}

func TestGoldenOutboundOptions(t *testing.T) {
	tests := []struct {
		name   string
		n      profile.Node
		assert func(*testing.T, any)
	}{
		{"shadowsocks2022", func() profile.Node {
			n := node(profile.ProtocolShadowsocks)
			n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA==", IdentityKeys: []string{"AQEBAQEBAQEBAQEBAQEBAQ=="}}
			return n
		}(), func(t *testing.T, v any) {
			o, ok := v.(*option.ShadowsocksOutboundOptions)
			if !ok || o.Method != "2022-blake3-aes-128-gcm" || o.Server != "8.8.8.8" || o.Password != "AQEBAQEBAQEBAQEBAQEBAQ==:AAAAAAAAAAAAAAAAAAAAAA==" {
				t.Fatalf("%#v", v)
			}
		}},
		{"vless-reality", func() profile.Node {
			n := node(profile.ProtocolVLESS)
			n.Ingresses[0].Credentials.VLESS = &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001", Flow: "xtls-rprx-vision"}
			n.Ingresses[0].TLS = &profile.TLS{ServerName: "edge.example.com", Reality: &profile.Reality{PublicKey: "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA", ShortID: "01"}}
			return n
		}(), func(t *testing.T, v any) {
			o, ok := v.(*option.VLESSOutboundOptions)
			if !ok || o.TLS == nil || o.TLS.Reality == nil || o.TLS.ServerName != "edge.example.com" {
				t.Fatalf("%#v", v)
			}
		}},
		{"anytls", func() profile.Node {
			n := node(profile.ProtocolAnyTLS)
			n.Ingresses[0].Credentials.AnyTLS = &profile.AnyTLSCredentials{Password: "pw"}
			n.Ingresses[0].TLS = &profile.TLS{ServerName: "edge.example.com"}
			return n
		}(), func(t *testing.T, v any) {
			o, ok := v.(*option.AnyTLSOutboundOptions)
			if !ok || o.Password != "pw" || o.TLS == nil {
				t.Fatalf("%#v", v)
			}
		}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := Build(base(tt.n), profile.PlatformCapabilities{LogLevel: "info"}, time.Now())
			if err != nil {
				t.Fatal(err)
			}
			if len(got.Options.Outbounds) != 2 {
				t.Fatal("outbound count")
			}
			selector, ok := got.Options.Outbounds[0].Options.(*option.SelectorOutboundOptions)
			if !ok || selector.Default != got.NodeTags[tt.n.ID] || selector.InterruptExistConnections {
				t.Fatalf("selector: %#v", got.Options.Outbounds[0].Options)
			}
			tt.assert(t, got.Options.Outbounds[1].Options)
		})
	}
}
func TestStableTagAcrossEndpointMigration(t *testing.T) {
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	a, _ := Build(base(n), profile.PlatformCapabilities{}, time.Now())
	n.Ingresses[0].Endpoint.Domain = "new.example.com"
	n.Ingresses[0].Endpoint.IP = "1.1.1.1"
	b, _ := Build(base(n), profile.PlatformCapabilities{}, time.Now())
	if a.NodeTags[n.ID] != b.NodeTags[n.ID] {
		t.Fatal("tag changed")
	}
}

// SIP022: identity_keys are the server iPSKs, outermost first, and user_key is
// the user's uPSK; the EIH password is iPSK1:...:iPSKn:uPSK.
func TestShadowsocksEIHPasswordOrder(t *testing.T) {
	const (
		ipsk1 = "AQEBAQEBAQEBAQEBAQEBAQ=="
		ipsk2 = "AgICAgICAgICAgICAgICAg=="
		upsk  = "AwMDAwMDAwMDAwMDAwMDAw=="
	)
	for _, tc := range []struct {
		identity []string
		want     string
	}{
		{nil, upsk},
		{[]string{ipsk1}, ipsk1 + ":" + upsk},
		{[]string{ipsk1, ipsk2}, ipsk1 + ":" + ipsk2 + ":" + upsk},
	} {
		in := ingress(profile.ProtocolShadowsocks, profile.IngressRolePrimary, "edge.example.com", "8.8.8.8")
		in.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", IdentityKeys: tc.identity, UserKey: upsk}
		out, err := buildOutbound(in, "tag")
		if err != nil {
			t.Fatal(err)
		}
		o, ok := out.Options.(*option.ShadowsocksOutboundOptions)
		if !ok || o.Password != tc.want {
			t.Fatalf("identity %v: %#v", tc.identity, out.Options)
		}
	}
}
