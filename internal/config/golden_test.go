package config

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/option"
	singjson "github.com/sagernet/sing/common/json"
)

func TestProfileToOptionsGolden(t *testing.T) {
	ss := node(profile.ProtocolShadowsocks)
	ss.ID = "ss"
	ss.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA==", IdentityKeys: []string{"AQEBAQEBAQEBAQEBAQEBAQ=="}}
	vless := node(profile.ProtocolVLESS)
	vless.ID = "vless"
	vless.Ingresses[0].Credentials.VLESS = &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001", Flow: "xtls-rprx-vision"}
	vless.Ingresses[0].TLS = &profile.TLS{ServerName: "edge.example.com", Reality: &profile.Reality{PublicKey: "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA", ShortID: "01"}}
	anytls := node(profile.ProtocolAnyTLS)
	anytls.ID = "anytls"
	anytls.Ingresses[0].Credentials.AnyTLS = &profile.AnyTLSCredentials{Password: "anytls-password"}
	anytls.Ingresses[0].TLS = &profile.TLS{ServerName: "edge.example.com"}
	p := &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "golden", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{ss, vless, anytls}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "ss"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
	built, err := Build(p, profile.PlatformCapabilities{LogLevel: "info"}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	raw, err := singjson.MarshalContext(failover.Context(context.Background()), built.Options)
	if err != nil {
		t.Fatal(err)
	}
	formatted := new(bytes.Buffer)
	if err = json.Indent(formatted, raw, "", "  "); err != nil {
		t.Fatal(err)
	}
	formatted.WriteByte('\n')
	if os.Getenv("UPDATE_GOLDEN") == "1" {
		if err = os.WriteFile("../../testdata/golden/options.json", formatted.Bytes(), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	expected, err := os.ReadFile("../../testdata/golden/options.json")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(formatted.Bytes(), expected) {
		t.Fatalf("golden mismatch\n--- got ---\n%s", formatted.Bytes())
	}
}

// TestMultiIngressProfileGolden renders the shared fixture (one node with a
// primary+backup, one single-ingress node) for desktop TUN plus local proxies,
// and checks the rendered JSON decodes again through the core's registry.
func TestMultiIngressProfileGolden(t *testing.T) {
	data, err := os.ReadFile("../../testdata/profiles/multi-ingress.json")
	if err != nil {
		t.Fatal(err)
	}
	p, err := profile.Parse(data)
	if err != nil {
		t.Fatal(err)
	}
	proxies := []localproxy.Endpoint{
		{NodeID: "jp-tyo-01", Listen: "127.0.0.1", Port: 20001, Username: "u1", Password: "p1"},
		{NodeID: "us-sjc-01", Listen: "127.0.0.1", Port: 20002, Username: "u2", Password: "p2"},
	}
	built, err := BuildWithLocalProxies(p, profile.PlatformCapabilities{Platform: "macos", TUN: profile.TUNCapabilities{Enabled: true}, LogLevel: "info"}, proxies, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	ctx := failover.Context(context.Background())
	raw, err := singjson.MarshalContext(ctx, built.Options)
	if err != nil {
		t.Fatal(err)
	}
	var decoded option.Options
	if err = singjson.UnmarshalContext(ctx, raw, &decoded); err != nil {
		t.Fatalf("rendered options do not decode: %v", err)
	}
	formatted := new(bytes.Buffer)
	if err = json.Indent(formatted, raw, "", "  "); err != nil {
		t.Fatal(err)
	}
	formatted.WriteByte('\n')
	const golden = "../../testdata/golden/options-multi-ingress.json"
	if os.Getenv("UPDATE_GOLDEN") == "1" {
		if err = os.WriteFile(golden, formatted.Bytes(), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	expected, err := os.ReadFile(golden)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(formatted.Bytes(), expected) {
		t.Fatalf("golden mismatch\n--- got ---\n%s", formatted.Bytes())
	}
	jp := built.NodeTags["jp-tyo-01"]
	if built.OutboundNodes[jp+"-0"] != "jp-tyo-01" || built.OutboundNodes[jp+"-1"] != "jp-tyo-01" || built.OutboundNodes[jp] != "jp-tyo-01" {
		t.Fatalf("outbound attribution: %#v", built.OutboundNodes)
	}
}
