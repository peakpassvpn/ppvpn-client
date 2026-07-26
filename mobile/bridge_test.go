package mobile

import (
	"encoding/json"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestBridgeDoesNotExposeNodeCredentials(t *testing.T) {
	platform, _ := json.Marshal(profile.PlatformCapabilities{Platform: "ios"})
	bridge, err := NewBridge(string(platform), filepath.Join(t.TempDir(), "state.json"))
	if err != nil {
		t.Fatal(err)
	}
	n := profile.Node{ID: "node", Name: "Tokyo", Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: "edge.example.com", IP: "8.8.8.8", Port: 443}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}}, Capabilities: profile.Capabilities{TCP: true}}
	p := profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "r", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{n}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
	data, _ := json.Marshal(p)
	if _, err = bridge.ApplyProfile(string(data)); err != nil {
		t.Fatal(err)
	}
	nodes, err := bridge.ListNodes()
	if err != nil || strings.Contains(nodes, "AAAAAAAAAAAAAAAAAAAAAA==") || strings.Contains(nodes, "credentials") {
		t.Fatalf("%v %s", err, nodes)
	}
}

func TestFlowIOTimeoutZeroMeansNoDeadline(t *testing.T) {
	if got := flowIODuration(0); got != 0 {
		t.Fatalf("zero timeout installed a deadline: %v", got)
	}
	if got := flowIODuration(-1); got != 0 {
		t.Fatalf("negative timeout installed a deadline: %v", got)
	}
	if got := flowIODuration(250); got != 250*time.Millisecond {
		t.Fatalf("positive timeout: %v", got)
	}
	if got := flowIODuration(999999); got != 120*time.Second {
		t.Fatalf("timeout cap: %v", got)
	}
}
