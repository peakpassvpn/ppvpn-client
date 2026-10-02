package runtime

import (
	"context"
	"errors"
	"path/filepath"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/probe"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/control"
)

// networkEngine is a fake engine with an injectable interface monitor state
// that captures the default interface change callback.
type networkEngine struct {
	fakeEngine
	known, present bool
	changed        func(*control.Interface)
}

func (e *networkEngine) defaultInterfaceState() (bool, bool) { return e.known, e.present }
func (e *networkEngine) watchDefaultInterface(_ *corelog.Logger, changed func(*control.Interface)) {
	e.changed = changed
}

func runningNetworkCore(t *testing.T, platform profile.PlatformCapabilities, known, present bool) (*Core, *networkEngine) {
	t.Helper()
	fake := &networkEngine{known: known, present: present}
	core := newCore(platform, func(context.Context, option.Options) (engine, error) { return fake, nil })
	core.proxyManager = localproxy.NewManager(filepath.Join(t.TempDir(), "proxy-state.json")).WithPreferredPort(0).WithSystemProxyPreferredPort(0)
	if _, err := core.ApplyProfile(testProfile("r1", "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = core.Stop() })
	return core, fake
}

// With no default interface (offline) both probes fail at once with
// ErrNoDefaultInterface instead of waiting out their timeout; with one, or
// when the engine cannot tell, they run.
func TestProbesFailFastWithoutDefaultInterface(t *testing.T) {
	platform := profile.PlatformCapabilities{Platform: "windows", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}}
	core, engine := runningNetworkCore(t, platform, true, false)
	started := time.Now()
	if _, err := core.ProbeEntrances(context.Background(), probe.MethodTCP, 5*time.Second, 1); !errors.Is(err, ErrNoDefaultInterface) {
		t.Fatalf("probe-entrances offline: %v", err)
	}
	if _, err := core.ProbeAvailability(context.Background(), "node", "http://example.com/", 5*time.Second); !errors.Is(err, ErrNoDefaultInterface) {
		t.Fatalf("probe-availability offline: %v", err)
	}
	if elapsed := time.Since(started); elapsed > time.Second {
		t.Fatalf("offline probes took %s", elapsed)
	}
	for _, state := range []struct{ known, present bool }{{true, true}, {false, false}} {
		engine.known, engine.present = state.known, state.present
		if _, err := core.ProbeEntrances(context.Background(), probe.MethodTCP, 50*time.Millisecond, 1); errors.Is(err, ErrNoDefaultInterface) {
			t.Fatalf("known=%v present=%v: failed fast", state.known, state.present)
		}
	}
}

// Every default interface change of the running engine is reported as
// NetworkChanged: the new interface's name and index, or none.
func TestDefaultInterfaceChangeEmitsNetworkChanged(t *testing.T) {
	core, engine := runningNetworkCore(t, profile.PlatformCapabilities{Platform: "windows", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}}, true, true)
	if engine.changed == nil {
		t.Fatal("the core did not watch the default interface")
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	events := core.Subscribe(ctx, 8)
	next := func() Event {
		t.Helper()
		for {
			select {
			case event := <-events:
				if event.Type == EventNetworkChanged {
					return event
				}
			case <-time.After(2 * time.Second):
				t.Fatal("no NetworkChanged event")
			}
		}
	}
	engine.changed(nil)
	if event := next(); event.HasDefaultInterface == nil || *event.HasDefaultInterface || event.InterfaceName != "" || event.InterfaceIndex != 0 {
		t.Fatalf("offline: %#v", event)
	}
	engine.changed(&control.Interface{Name: "Ethernet", Index: 7})
	if event := next(); event.HasDefaultInterface == nil || !*event.HasDefaultInterface || event.InterfaceName != "Ethernet" || event.InterfaceIndex != 7 {
		t.Fatalf("back online: %#v", event)
	}
}
