package runtime

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

// TestProfileRejectRuleDoesNotCrashCore hits a profile reject rule on a real
// sing-box. Options go to box.New without JSON decoding, so a reject action
// without an explicit method used to panic the whole process on first match.
func TestProfileRejectRuleDoesNotCrashCore(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })
	blocked := httptest.NewServer(handler)
	defer blocked.Close()
	allowed := httptest.NewServer(handler)
	defer allowed.Close()
	serverPort := startShadowsocksServer(t)
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "reject", ExpiresAt: time.Now().Add(time.Hour),
		Nodes:     []profile.Node{{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "n0", 0, serverPort)}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"},
		Routing: profile.Routing{
			Rules: []profile.RoutingRule{
				{ID: "block", Match: profile.RoutingMatch{Ports: []uint16{targetPort(t, blocked)}}, Action: profile.RoutingAction{Type: "reject"}},
				{ID: "allow", Match: profile.RoutingMatch{Ports: []uint16{targetPort(t, allowed)}}, Action: profile.RoutingAction{Type: "direct"}},
			},
			Final: profile.RoutingAction{Type: "reject"},
		},
	}
	core := newLocalProxyTestCore(t, profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"})
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	status, err := core.SetSystemProxy(true)
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 2; i++ {
		if code, err := systemProxyGet(t, status, blocked.URL); err == nil && code == http.StatusNoContent {
			t.Fatal("rejected destination was reachable")
		}
		if code, err := systemProxyGet(t, status, allowed.URL); err != nil || code != http.StatusNoContent {
			t.Fatalf("core stopped serving after a reject: %d %v", code, err)
		}
	}
}
