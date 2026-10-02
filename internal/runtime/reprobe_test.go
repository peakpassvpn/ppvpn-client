package runtime

import (
	"context"
	"github.com/sagernet/sing/common/control"
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
	changed func(*control.Interface)
}

func (w *watchSwap) watchDefaultInterface(_ *corelog.Logger, changed func(*control.Interface)) {
	w.changed = changed
}

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

// manualTimer stands in for time.AfterFunc: the test fires the pending
// probe itself, so no test depends on how fast the machine runs.
type manualTimer struct {
	mu        sync.Mutex
	pending   func()
	scheduled int
	stopped   int
	delay     time.Duration
}

func (m *manualTimer) after(d time.Duration, f func()) func() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.scheduled++
	m.delay = d
	var done bool
	m.pending = func() {
		if !done {
			done = true
			f()
		}
	}
	return func() bool {
		m.mu.Lock()
		defer m.mu.Unlock()
		if done {
			return false
		}
		done = true
		m.stopped++
		return true
	}
}

// fire runs the pending probe, if one is still armed.
func (m *manualTimer) fire() {
	m.mu.Lock()
	f := m.pending
	m.pending = nil
	m.mu.Unlock()
	if f != nil {
		f()
	}
}

type reprobeRig struct {
	core   *Core
	engine *watchSwap
	log    *lockedLog
	timer  *manualTimer
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
	r := &reprobeRig{log: &lockedLog{}, route: route, timer: &manualTimer{}}
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
	r.core.reprobeAfter = r.timer.after
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

// The host loses its IPv6 path (joins an IPv4-only network): one kernel
// switch to the hand-off build, no restart, armed ReprobeDelay out.
func TestReprobeSwitchesWhenIPv6PathIsLost(t *testing.T) {
	r := newReprobeRig(t, true)
	r.setRoute(false)
	r.engine.changed(nil)
	if r.timer.delay != ReprobeDelay {
		t.Fatalf("probe armed %v out, want %v", r.timer.delay, ReprobeDelay)
	}
	if got, _, _ := r.swaps(); got != 0 {
		t.Fatalf("swapped %d times before the delay", got)
	}
	r.timer.fire()
	if got, restarts, handOff := r.swaps(); got != 1 || restarts != "" || !handOff {
		t.Fatalf("swaps %d, restart reasons %q, hand-off %v", got, restarts, handOff)
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
	r.engine.changed(nil)
	r.timer.fire()
	if got, restarts, handOff := r.swaps(); got != 1 || restarts != "" || handOff {
		t.Fatalf("swaps %d, restart reasons %q, hand-off %v", got, restarts, handOff)
	}
	if !strings.Contains(r.log.String(), `policy=tun_ipv6_direct_ipv4 policy=tun_ipv6 rebuilt=true switch=kernel`) {
		t.Fatalf("log:\n%s", r.log.String())
	}
}

// Same result: nothing rebuilt, no change logged.
func TestReprobeKeepsKernelWhenUnchanged(t *testing.T) {
	r := newReprobeRig(t, false)
	r.engine.changed(nil)
	r.timer.fire()
	if got, _, _ := r.swaps(); got != 0 {
		t.Fatalf("swaps %d", got)
	}
	if strings.Contains(r.log.String(), "host ipv6 changed") {
		t.Fatalf("log:\n%s", r.log.String())
	}
}

// A burst of changes (a Wi-Fi switch) re-arms one probe: each change stops
// the pending one, and only the last fires.
func TestReprobeDebouncesBursts(t *testing.T) {
	r := newReprobeRig(t, true)
	r.setRoute(false)
	for range 5 {
		r.engine.changed(nil)
	}
	if r.timer.scheduled != 5 || r.timer.stopped != 4 {
		t.Fatalf("scheduled %d, stopped %d; want 5 and 4", r.timer.scheduled, r.timer.stopped)
	}
	before := strings.Count(r.log.String(), `msg="host ipv6"`)
	r.timer.fire()
	r.timer.fire() // nothing is left to fire
	if got, _, _ := r.swaps(); got != 1 {
		t.Fatalf("swaps %d, want 1", got)
	}
	if probes := strings.Count(r.log.String(), `msg="host ipv6"`) - before; probes != 2 {
		// One probe by the re-probe, one by the rebuild it triggers.
		t.Fatalf("%d probes after the burst:\n%s", probes, r.log.String())
	}
}

// Stop cancels a pending re-probe.
func TestStopCancelsPendingReprobe(t *testing.T) {
	r := newReprobeRig(t, true)
	r.setRoute(false)
	r.engine.changed(nil)
	if err := r.core.Stop(); err != nil {
		t.Fatal(err)
	}
	if r.timer.stopped != 1 {
		t.Fatalf("pending probe not stopped (stopped %d)", r.timer.stopped)
	}
	r.timer.fire()
	if got, _, _ := r.swaps(); got != 0 {
		t.Fatalf("swaps %d after stop", got)
	}
}

// An apply between the change and the probe builds for the new state; the
// probe then finds nothing to do.
func TestReprobeDefersToApply(t *testing.T) {
	r := newReprobeRig(t, true)
	r.setRoute(false)
	r.engine.changed(nil)
	if _, err := r.core.ApplyProfile(testProfile("user-apply", "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	r.timer.fire()
	if got, restarts, handOff := r.swaps(); got != 1 || restarts != "" || !handOff {
		t.Fatalf("swaps %d (want the apply's only), restart reasons %q, hand-off %v", got, restarts, handOff)
	}
	if strings.Contains(r.log.String(), "host ipv6 changed") {
		t.Fatalf("re-probe rebuilt after the apply:\n%s", r.log.String())
	}
}

// The default scheduler is time.AfterFunc: a change still leads to a probe
// on its own (with a short delay and a generous deadline).
func TestReprobeRealTimerFires(t *testing.T) {
	r := newReprobeRig(t, true)
	r.core.reprobeAfter = newCore(profile.PlatformCapabilities{}, nil).reprobeAfter
	r.core.reprobeDelay = 10 * time.Millisecond
	r.setRoute(false)
	r.engine.changed(nil)
	deadline := time.Now().Add(10 * time.Second)
	for {
		if got, _, _ := r.swaps(); got == 1 {
			return
		}
		if time.Now().After(deadline) {
			t.Fatal("the real timer never probed")
		}
		time.Sleep(5 * time.Millisecond)
	}
}
