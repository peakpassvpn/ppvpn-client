package config

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/include"
	singjson "github.com/sagernet/sing/common/json"
)

func TestProfileToOptionsGolden(t *testing.T) {
	ss := node(profile.ProtocolShadowsocks)
	ss.ID = "ss"
	ss.Credentials.Shadowsocks = &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA==", IdentityKeys: []string{"AQEBAQEBAQEBAQEBAQEBAQ=="}}
	vless := node(profile.ProtocolVLESS)
	vless.ID = "vless"
	vless.Credentials.VLESS = &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001", Flow: "xtls-rprx-vision"}
	vless.TLS = &profile.TLS{ServerName: "edge.example.com", Reality: &profile.Reality{PublicKey: "public-key", ShortID: "01"}}
	anytls := node(profile.ProtocolAnyTLS)
	anytls.ID = "anytls"
	anytls.Credentials.AnyTLS = &profile.AnyTLSCredentials{Password: "anytls-password"}
	anytls.TLS = &profile.TLS{ServerName: "edge.example.com"}
	p := &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "golden", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{ss, vless, anytls}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "ss"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
	built, err := Build(p, profile.PlatformCapabilities{LogLevel: "info"}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	raw, err := singjson.MarshalContext(include.Context(context.Background()), built.Options)
	if err != nil {
		t.Fatal(err)
	}
	formatted := new(bytes.Buffer)
	if err = json.Indent(formatted, raw, "", "  "); err != nil {
		t.Fatal(err)
	}
	formatted.WriteByte('\n')
	expected, err := os.ReadFile("../../testdata/golden/options.json")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(formatted.Bytes(), expected) {
		t.Fatalf("golden mismatch\n--- got ---\n%s", formatted.Bytes())
	}
}
