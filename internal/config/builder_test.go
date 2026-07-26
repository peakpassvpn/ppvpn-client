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
	return profile.Node{ID: "stable", Protocol: protocol, Endpoint: profile.Endpoint{Domain: "edge.example.com", IP: "8.8.8.8", Port: 443}, Capabilities: profile.Capabilities{TCP: true}}
}
func TestGoldenOutboundOptions(t *testing.T) {
	tests := []struct {
		name   string
		n      profile.Node
		assert func(*testing.T, any)
	}{
		{"shadowsocks2022", func() profile.Node {
			n := node(profile.ProtocolShadowsocks)
			n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA==", IdentityKeys: []string{"AQEBAQEBAQEBAQEBAQEBAQ=="}}
			return n
		}(), func(t *testing.T, v any) {
			o, ok := v.(*option.ShadowsocksOutboundOptions)
			if !ok || o.Method != "2022-blake3-aes-128-gcm" || o.Server != "edge.example.com" || o.Password != "AQEBAQEBAQEBAQEBAQEBAQ==:AAAAAAAAAAAAAAAAAAAAAA==" {
				t.Fatalf("%#v", v)
			}
		}},
		{"vless-reality", func() profile.Node {
			n := node(profile.ProtocolVLESS)
			n.Credentials.VLESS = &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001", Flow: "xtls-rprx-vision"}
			n.TLS = &profile.TLS{ServerName: "edge.example.com", Reality: &profile.Reality{PublicKey: "pk", ShortID: "01"}}
			return n
		}(), func(t *testing.T, v any) {
			o, ok := v.(*option.VLESSOutboundOptions)
			if !ok || o.TLS == nil || o.TLS.Reality == nil || o.TLS.ServerName != "edge.example.com" {
				t.Fatalf("%#v", v)
			}
		}},
		{"anytls", func() profile.Node {
			n := node(profile.ProtocolAnyTLS)
			n.Credentials.AnyTLS = &profile.AnyTLSCredentials{Password: "pw"}
			n.TLS = &profile.TLS{ServerName: "edge.example.com"}
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
	n.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	a, _ := Build(base(n), profile.PlatformCapabilities{}, time.Now())
	n.Endpoint.Domain = "new.example.com"
	n.Endpoint.IP = "1.1.1.1"
	b, _ := Build(base(n), profile.PlatformCapabilities{}, time.Now())
	if a.NodeTags[n.ID] != b.NodeTags[n.ID] {
		t.Fatal("tag changed")
	}
}
