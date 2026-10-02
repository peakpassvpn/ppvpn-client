package runtime

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

// A pinned node sends every connection through its pinned ingress with no
// fallback, the pin takes effect on the running engine and survives start,
// status reports it with the ingresses, and a profile without the pinned
// ingress clears it.
func TestPinIngressOnARunningCore(t *testing.T) {
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }))
	defer target.Close()
	primaryHits, backupHits := &destinationRecorder{}, &destinationRecorder{}
	primaryPort := startShadowsocksServerWithTracker(t, primaryHits)
	backupPort := startShadowsocksServerWithTracker(t, backupHits)
	deadPort := deadPort(t)
	targetAddress := target.Listener.Addr().String()

	build := func(revision string, ingresses ...profile.Ingress) *profile.Profile {
		return &profile.Profile{
			SchemaVersion: profile.CurrentSchemaVersion, Revision: revision, ExpiresAt: time.Now().Add(time.Hour),
			Nodes:     []profile.Node{{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: ingresses}},
			Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"},
			Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
		}
	}
	a := localSSIngress(profile.IngressRolePrimary, "a", 0, primaryPort)
	b := localSSIngress(profile.IngressRoleBackup, "b", 1, backupPort)
	c := localSSIngress(profile.IngressRoleBackup, "c", 2, deadPort)
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err := core.ApplyProfile(build("r1", a, b, c), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.PinIngress("nope", "b"); !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("unknown node: %v", err)
	}
	if err := core.PinIngress("node", "z"); !errors.Is(err, ErrIngressNotFound) {
		t.Fatalf("unknown ingress: %v", err)
	}
	// Pinned before start: the first connection already uses the backup.
	if err := core.PinIngress("node", "b"); err != nil {
		t.Fatal(err)
	}
	events := core.Subscribe(t.Context(), 32)
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	probe := func() bool {
		t.Helper()
		result, err := core.ProbeAvailability(context.Background(), "node", target.URL, 5*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		return result.Success
	}
	if !probe() || !backupHits.has(targetAddress) || primaryHits.has(targetAddress) {
		t.Fatalf("pinned to b: primary %v, backup %v", primaryHits.has(targetAddress), backupHits.has(targetAddress))
	}
	nodes := core.Status().Nodes
	if len(nodes) != 1 || nodes[0].PinnedEndpointKey == nil || *nodes[0].PinnedEndpointKey != "b" || len(nodes[0].Ingresses) != 3 ||
		!nodes[0].Ingresses[1].Active || nodes[0].Ingresses[0].Active || nodes[0].Ingresses[1].Healthy == nil {
		t.Fatalf("status: %+v", nodes)
	}

	// Pinned to a dead ingress: the probe fails and nothing falls back.
	primaryHits.reset()
	backupHits.reset()
	if err := core.PinIngress("node", "c"); err != nil {
		t.Fatal(err)
	}
	if probe() || primaryHits.has(targetAddress) || backupHits.has(targetAddress) {
		t.Fatal("pinned dead ingress fell back to another one")
	}

	// Automatic again: the primary serves.
	if err := core.PinIngress("node", ""); err != nil {
		t.Fatal(err)
	}
	if !probe() || !primaryHits.has(targetAddress) {
		t.Fatal("automatic selection did not return to the primary")
	}
	if nodes = core.Status().Nodes; nodes[0].PinnedEndpointKey != nil {
		t.Fatalf("pin reported after unpin: %+v", nodes[0])
	}

	// A new revision without the pinned ingress drops the pin.
	if err := core.PinIngress("node", "c"); err != nil {
		t.Fatal(err)
	}
	if _, err := core.ApplyProfile(build("r2", a, b), time.Now()); err != nil {
		t.Fatal(err)
	}
	if nodes = core.Status().Nodes; nodes[0].PinnedEndpointKey != nil || len(nodes[0].Ingresses) != 2 {
		t.Fatalf("stale pin kept: %+v", nodes[0])
	}
	deadline := time.After(2 * time.Second)
	for cleared := false; !cleared; {
		select {
		case event := <-events:
			cleared = event.Type == EventNodeIngressPinCleared && event.NodeID == "node" && event.EndpointKey == "c" && event.Revision == "r2"
		case <-deadline:
			t.Fatal("no NodeIngressPinCleared event")
		}
	}
}
