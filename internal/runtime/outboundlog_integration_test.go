package runtime

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

// At debug level a failed node connection names the node, the ingress, the
// protocol, the stage and the error, and never the credentials: a dead port
// fails at dial; a Shadowsocks key the server rejects only shows as the
// connection closing before any response.
func TestOutboundFailuresAreLoggedAtDebug(t *testing.T) {
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }))
	defer target.Close()
	serverPort := startShadowsocksServer(t)
	wrongKey := "Zm9vYmFyYmF6cXV4MTIzNA=="
	rejected := localSSIngress(profile.IngressRolePrimary, "wrong-key", 0, serverPort)
	rejected.Credentials.Shadowsocks.UserKey = wrongKey
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "outbound-log", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{
			{ID: "dead", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
				localSSIngress(profile.IngressRolePrimary, "dead-port", 0, freePort(t)),
			}},
			{ID: "rejected", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{rejected}},
		},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "dead"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	var mu sync.Mutex
	var b strings.Builder
	log := corelog.New(writerFunc(func(data []byte) (int, error) { mu.Lock(); defer mu.Unlock(); return b.Write(data) }))
	_ = log.SetLevel(corelog.LevelDebug)
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	core.SetLogger(log)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	nodes := []string{"dead", "rejected"}
	if raceEnabled {
		// A key the server rejects closes the connection while the client
		// is still writing its lazy request header, which trips a data race
		// inside sing-shadowsocks2 (shadowaead_2022.clientConn: writeRequest
		// vs Close), not in this code. Only the dial failure runs under -race.
		nodes = nodes[:1]
	}
	for _, node := range nodes {
		if result, err := core.ProbeAvailability(context.Background(), node, target.URL, 3*time.Second); err != nil || result.Success {
			t.Fatalf("%s: %v %+v", node, err, result)
		}
	}
	deadline := time.Now().Add(2 * time.Second)
	for {
		mu.Lock()
		logged := b.String()
		mu.Unlock()
		dial := strings.Contains(logged, `msg="outbound failed" stage=dial node_id=dead endpoint_key=dead-port`) && strings.Contains(logged, "protocol=shadowsocks")
		closed := raceEnabled || strings.Contains(logged, `msg="outbound failed" stage="closed before any response" node_id=rejected endpoint_key=wrong-key`)
		if dial && closed {
			if strings.Contains(logged, wrongKey) || strings.Contains(logged, testSSKey) {
				t.Fatalf("credentials logged:\n%s", logged)
			}
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("dial=%v closed=%v in:\n%s", dial, closed, logged)
		}
		time.Sleep(20 * time.Millisecond)
	}
}
