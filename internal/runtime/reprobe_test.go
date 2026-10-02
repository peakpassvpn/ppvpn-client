package runtime

import (
	"context"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/option"
)

// watchSwap is a kernel-swapping engine whose default interface changes the
// test fires by hand.
type watchSwap struct {
	*fakeSwap
	changed func()
}

func (w *watchSwap) watchDefaultInterface(_ *corelog.Logger, changed func()) { w.changed = changed }

// lockedLog is a log sink safe to read while the core writes from a timer.
type lockedLog struct {
	mu sync.Mutex
	b  strings.Builder
}

func (l *lockedLog) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.Write(p)
}
func (l *lockedLog) String() string { l.mu.Lock(); defer l.mu.Unlock(); return l.b.String() }

type reprobeRig struct {
	core   *Core
	engine *watchSwap
	log    *lockedLog
	mu     sync.Mutex
	route  bool
}

func (r *reprobeRig) setRoute(route bool) { r.mu.Lock(); r.route = route; r.mu.Unlock() }

// swaps reads the engine under the operation lock the re-probe holds.
func (r *reprobeRig) swaps() (int, string, bool) {
	r.core.operation.Lock()
	defer r.core.operation.Unlock()
	return r.engine.swaps, strings.Join(r.engine.restarts, ""), r.core.built.DirectIPv6HandOff
}

func newReprobeRig(t *testing.T, route bool) *reprobeRig {
	t.Helper()
	r := &reprobeRig{log: &lockedLog{}, route: route}
	var engines int
	r.core = newCore(profile.PlatformCapabilities{Platform: "windows", TUN: profile.TUNCapabilities{Enabled: true}}, func(_ context.Context, options option.Options) (engine, error) {
		engines++
		if engines > 1 {
			t.Errorf("engine %d created: a full restart", engines)
		}
		r.engine = &watchSwap{fakeSwap: &fakeSwap{options: options}}
		return r.engine, nil
	})
	r.core.SetLogger(corelog.New(r.log))
	r.core.reprobeDelay = 30 * time.Millisecond
	r.core.hostIPv6 = func() bool { return true }
	r.core.hostIPv6Route = func() (bool, error) { r.mu.Lock(); defer r.mu.Unlock(); return r.route, nil }
	if _, err := r.core.ApplyProfile(testProfile("reprobe", "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := r.core.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = r.core.Stop() })
	if r.engine.changed == nil {
		t.Fatal("start did not watch the default interface")
	}
	return r
}

// waitSwaps waits until the engine swapped want times, then a little longer
// to see it does not swap again.
func (r *reprobeRig) waitSwaps(t *testing.T, want int) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for {
		got, _, _ := r.swaps()
		if got == want {
			time.Sleep(10 * r.core.reprobeDelay)
			if again, _, _ := r.swaps(); again != want {
				t.Fatalf("swaps %d, want %d", again, want)
			}
			return
		}
		if got > want || time.Now().After(deadline) {
			t.Fatalf("swaps %d, want %d", got, want)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

// The host loses its IPv6 path (joins an IPv4-only network): one kernel
// switch to the hand-off build, no restart.
func TestReprobeSwitchesWhenIPv6PathIsLost(t *testing.T) {
	r := newReprobeRig(t, true)
	r.setRoute(false)
	r.engine.changed()
	r.waitSwaps(t, 1)
	if _, restarts, handOff := r.swaps(); restarts != "" || !handOff {
		t.Fatalf("restart reasons %q, hand-off %v", restarts, handOff)
	}
	want := `msg="host ipv6 changed" previous_host_ipv6_route=true host_ipv6_route=false previous_policy=tun_ipv6 policy=tun_ipv6_direct_ipv4 rebuilt=true switch=kernel`
	if !strings.Contains(r.log.String(), want) {
		t.Fatalf("missing %q in:\n%s", want, r.log.String())
	}
}

// The host gains an IPv6 path: switch back to the plain build.
func TestReprobeSwitchesWhenIPv6PathAppears(t *testing.T) {
	r := newReprobeRig(t, false)
	r.setRoute(true)
	r.engine.changed()
	r.waitSwaps(t, 1)
	if _, restarts, handOff := r.swaps(); restarts != "" || handOff {
		t.Fatalf("restart reasons %q, hand-off %v", restarts, handOff)
	}
	if !strings.Contains(r.log.String(), `policy=tun_ipv6_direct_ipv4 policy=tun_ipv6 rebuilt=true switch=kernel`) {
		t.Fatalf("log:\n%s", r.log.String())
	}
}

// Same result: nothing rebuilt, no change logged.
func TestReprobeKeepsKernelWhenUnchanged(t *testing.T) {
	r := newReprobeRig(t, false)
	r.engine.changed()
	r.waitSwaps(t, 0)
	if strings.Contains(r.log.String(), "host ipv6 changed") {
		t.Fatalf("log:\n%s", r.log.String())
	}
}

// A burst of changes (a Wi-Fi switch) leads to one probe and one switch.
func TestReprobeDebouncesBursts(t *testing.T) {
	r := newReprobeRig(t, true)
	before := strings.Count(r.log.String(), `msg="host ipv6"`)
	r.setRoute(false)
	for range 5 {
		r.engine.changed()
		time.Sleep(r.core.reprobeDelay / 3)
	}
	r.waitSwaps(t, 1)
	if probes := strings.Count(r.log.String(), `msg="host ipv6"`) - before; probes != 2 {
		// One probe by the re-probe, one by the rebuild it triggers.
		t.Fatalf("%d probes after the burst:\n%s", probes, r.log.String())
	}
}

// Stop cancels a pending re-probe.
func TestStopCancelsPendingReprobe(t *testing.T) {
	r := newReprobeRig(t, true)
	r.setRoute(false)
	r.engine.changed()
	if err := r.core.Stop(); err != nil {
		t.Fatal(err)
	}
	time.Sleep(5 * r.core.reprobeDelay)
	if got, _, _ := r.swaps(); got != 0 {
		t.Fatalf("swaps %d after stop", got)
	}
}

// An apply between the change and the probe builds for the new state; the
// probe then finds nothing to do.
func TestReprobeDefersToApply(t *testing.T) {
	r := newReprobeRig(t, true)
	r.core.reprobeDelay = 200 * time.Millisecond
	r.setRoute(false)
	r.engine.changed()
	if _, err := r.core.ApplyProfile(testProfile("user-apply", "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	time.Sleep(2 * r.core.reprobeDelay)
	if got, restarts, handOff := r.swaps(); got != 1 || restarts != "" || !handOff {
		t.Fatalf("swaps %d (want the apply's only), restart reasons %q, hand-off %v", got, restarts, handOff)
	}
	if strings.Contains(r.log.String(), "host ipv6 changed") {
		t.Fatalf("re-probe rebuilt after the apply:\n%s", r.log.String())
	}
}
