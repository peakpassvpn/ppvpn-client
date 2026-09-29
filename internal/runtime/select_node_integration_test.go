package runtime

import (
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/adapter"
)

func selectorNow(t *testing.T, core *Core) string {
	t.Helper()
	core.mu.RLock()
	instance := core.engine.(*singEngine)
	core.mu.RUnlock()
	outbound, ok := instance.Outbound().Outbound("selected")
	if !ok {
		t.Fatal("selector outbound missing")
	}
	return outbound.(adapter.OutboundGroup).Now()
}

// TestStandardCoreSelectNode drives select-node on a real non-TUN core, both
// before start (the selection must survive Start) and while running.
func TestStandardCoreSelectNode(t *testing.T) {
	serverPort := startShadowsocksServer(t)
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "select-1", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{
			{ID: "first", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "f0", 0, serverPort)}},
			{ID: "second", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "s0", 0, serverPort)}},
		},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "first"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.SelectNode("second"); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	if got, want := selectorNow(t, core), core.built.NodeTags["second"]; got != want || core.Status().SelectedNodeID != "second" {
		t.Fatalf("selection before start lost: selector=%s want=%s status=%#v", got, want, core.Status())
	}
	if err := core.SelectNode("first"); err != nil {
		t.Fatal(err)
	}
	if got, want := selectorNow(t, core), core.built.NodeTags["first"]; got != want {
		t.Fatalf("running selection: %s want %s", got, want)
	}
	if err := core.Stop(); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	if got, want := selectorNow(t, core), core.built.NodeTags["first"]; got != want {
		t.Fatalf("selection lost across restart: %s want %s", got, want)
	}
}
