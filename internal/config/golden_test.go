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
	ss.Ingresses[0].EndpointKey = "ss-9001"
	ss.Ingresses[0].Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA==", IdentityKeys: []string{"AQEBAQEBAQEBAQEBAQEBAQ=="}}
	vless := node(profile.ProtocolVLESS)
	vless.ID = "vless"
	vless.Ingresses[0].EndpointKey = "vless-9001"
	vless.Ingresses[0].Credentials.VLESS = &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001", Flow: "xtls-rprx-vision"}
	vless.Ingresses[0].TLS = &profile.TLS{ServerName: "edge.example.com", Reality: &profile.Reality{PublicKey: "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA", ShortID: "01"}}
	anytls := node(profile.ProtocolAnyTLS)
	anytls.ID = "anytls"
	anytls.Ingresses[0].EndpointKey = "anytls-9001"
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
		{NodeID: jpNode, Listen: "127.0.0.1", Port: 7890, Username: localproxy.FormatUsername("u8f2k", jpNode), Password: "shared"},
		{NodeID: usNode, Listen: "127.0.0.1", Port: 7890, Username: localproxy.FormatUsername("u8f2k", usNode), Password: "shared"},
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
	jp := built.NodeTags[jpNode]
	primary, backup := ingressTag(jpNode, "9001"), ingressTag(jpNode, "9002")
	if built.OutboundNodes[primary] != jpNode || built.OutboundNodes[backup] != jpNode || built.OutboundNodes[jp] != jpNode {
		t.Fatalf("outbound attribution: %#v", built.OutboundNodes)
	}
}

const (
	jpNode = "3f2c9a1e-0000-4000-8000-000000000001-128"
	usNode = "3f2c9a1e-0000-4000-8000-000000000002-129"
)

// TestIngressTagsStableAcrossReorder checks member tags follow endpoint_key,
// not array position, so a reordered/extended replica set keeps existing tags.
func TestIngressTagsStableAcrossReorder(t *testing.T) {
	a := ingress(profile.ProtocolShadowsocks, profile.IngressRolePrimary, "a.example.com", "")
	a.EndpointKey, a.ReplicaOrdinal = "9001", 0
	b := ingress(profile.ProtocolShadowsocks, profile.IngressRoleBackup, "b.example.com", "")
	b.EndpointKey, b.ReplicaOrdinal = "9002", 1
	n := node(profile.ProtocolShadowsocks)
	n.Ingresses = []profile.Ingress{a, b}
	first, err := Build(base(n), profile.PlatformCapabilities{}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	a.Role, b.Role = profile.IngressRoleBackup, profile.IngressRolePrimary
	a.ReplicaOrdinal, b.ReplicaOrdinal = 1, 0
	n.Ingresses = []profile.Ingress{b, a}
	second, err := Build(base(n), profile.PlatformCapabilities{}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	members := func(r *BuildResult) []string {
		for _, o := range r.Options.Outbounds {
			if o.Type == failover.Type {
				return o.Options.(*failover.Options).Outbounds
			}
		}
		t.Fatal("no failover group")
		return nil
	}
	m1, m2 := members(first), members(second)
	if len(m1) != 2 || m1[0] != m2[1] || m1[1] != m2[0] || m1[0] != ingressTag("stable", "9001") {
		t.Fatalf("tags not keyed by endpoint_key: %v vs %v", m1, m2)
	}
}
