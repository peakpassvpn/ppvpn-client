package runtime

import (
	"bufio"
	"context"
	"errors"
	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/proxyinbound"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"os"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

// shortDrain makes replaced kernels drain fast in a test.
func shortDrain(t *testing.T, limit time.Duration) {
	t.Helper()
	grace, interval, previous := drainGrace, drainCheckInterval, drainLimit
	drainGrace, drainCheckInterval, drainLimit = 100*time.Millisecond, 20*time.Millisecond, limit
	t.Cleanup(func() { drainGrace, drainCheckInterval, drainLimit = grace, interval, previous })
}

// hotSwap is a running local-proxy core with two Shadowsocks nodes, a slow
// download target and a quick one, and the events it emits.
type hotSwap struct {
	core           *Core
	slowPort       uint16
	slow, quick    *httptest.Server
	portA, portB   uint16
	events         <-chan Event
	pending        []Event // received while waiting for another type
	release        chan struct{}
	releaseOnce    sync.Once
	log            *strings.Builder
	logMu          *sync.Mutex
	downloadChunks int
}

const chunk = 64 << 10

func newHotSwap(t *testing.T) *hotSwap {
	t.Helper()
	f := &hotSwap{release: make(chan struct{}), downloadChunks: 64, logMu: &sync.Mutex{}, log: &strings.Builder{}}
	// The slow target streams 64 chunks of 64 KiB, the second half only once
	// released, so a download is mid-way when a test applies a profile.
	f.slow = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Length", strconv.Itoa(f.downloadChunks*chunk))
		data := make([]byte, chunk)
		for i := range f.downloadChunks {
			if i == f.downloadChunks/2 {
				select {
				case <-f.release:
				case <-time.After(10 * time.Second):
					return
				}
			}
			if _, err := w.Write(data); err != nil {
				return
			}
			w.(http.Flusher).Flush()
		}
	}))
	t.Cleanup(f.slow.Close)
	f.quick = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }))
	t.Cleanup(f.quick.Close)
	// Cleanups run last-registered first: release before the servers close,
	// or a waiting handler holds slow.Close until its timeout.
	t.Cleanup(f.releaseAll)
	f.slowPort = targetPort(t, f.slow)
	f.portA, f.portB = startShadowsocksServer(t), startShadowsocksServer(t)
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	f.core = newLocalProxyTestCore(t, platform)
	logger := corelog.New(writerFunc(func(p []byte) (int, error) {
		f.logMu.Lock()
		defer f.logMu.Unlock()
		return f.log.Write(p)
	}))
	f.core.SetLogger(logger)
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	f.events = f.core.Subscribe(ctx, 64)
	if _, err := f.core.ApplyProfile(f.profile("r1", []string{"a", "b"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := f.core.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = f.core.Stop() })
	return f
}

// releaseAll lets every slow download send its second half.
func (f *hotSwap) releaseAll() { f.releaseOnce.Do(func() { close(f.release) }) }

func (f *hotSwap) logged() string {
	f.logMu.Lock()
	defer f.logMu.Unlock()
	return f.log.String()
}

// profile has the given nodes and rules; the final routes to the selected
// node.
func (f *hotSwap) profile(revision string, nodes []string, rules []profile.RoutingRule) *profile.Profile {
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: revision, ExpiresAt: time.Now().Add(time.Hour),
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: nodes[0]},
		Routing:   profile.Routing{Rules: rules, Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	ports := map[string]uint16{"a": f.portA, "b": f.portB}
	for _, id := range nodes {
		p.Nodes = append(p.Nodes, profile.Node{ID: id, EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, id+"0", 0, ports[id])}})
	}
	return p
}

func (f *hotSwap) user(t *testing.T, nodeID string) (string, string) {
	t.Helper()
	for _, endpoint := range f.core.LocalProxyEndpoints() {
		if endpoint.NodeID == nodeID {
			return endpoint.Username, endpoint.Password
		}
	}
	routed, err := f.core.LocalProxyRoutedCredential()
	if err != nil || nodeID != "" {
		t.Fatalf("no local proxy user for %q: %v", nodeID, err)
	}
	return routed.Username, routed.Password
}

// startDownload opens a CONNECT tunnel as user to the slow target and reads
// its first half; the result channel gets the bytes of the body read in the
// end, or the error.
func (f *hotSwap) startDownload(t *testing.T, username, password string) (net.Conn, <-chan error) {
	t.Helper()
	endpoint := f.core.LocalProxyEndpoints()[0]
	tunnel, status, err := httpConnect(endpoint, username, password, f.slow.Listener.Addr().String())
	if err != nil || status[:3] != "200" {
		t.Fatalf("CONNECT: %q %v", status, err)
	}
	_ = tunnel.SetDeadline(time.Now().Add(30 * time.Second))
	if _, err = io.WriteString(tunnel, "GET / HTTP/1.1\r\nHost: slow\r\n\r\n"); err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(tunnel)
	response, err := http.ReadResponse(reader, nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = io.ReadFull(response.Body, make([]byte, f.downloadChunks/2*chunk)); err != nil {
		t.Fatalf("first half: %v", err)
	}
	done := make(chan error, 1)
	go func() {
		n, err := io.Copy(io.Discard, response.Body)
		switch {
		case err != nil:
			done <- err
		case int(n) != f.downloadChunks/2*chunk:
			done <- errors.New("short body: " + strconv.FormatInt(n, 10))
		default:
			done <- nil
		}
	}()
	return tunnel, done
}

func (f *hotSwap) quickGet(t *testing.T, username, password string) error {
	t.Helper()
	endpoint := f.core.LocalProxyEndpoints()[0]
	tunnel, status, err := httpConnect(endpoint, username, password, f.quick.Listener.Addr().String())
	if err != nil {
		return err
	}
	defer tunnel.Close()
	if status[:3] != "200" {
		return errors.New(status)
	}
	code, err := tunnelGet(tunnel, f.quick.Listener.Addr().String())
	if err == nil && code != http.StatusNoContent {
		err = errors.New(strconv.Itoa(code))
	}
	return err
}

// next returns the next event of type. Events of other types received
// meanwhile are kept for later calls, not dropped (a drain may land while a
// test waits for a switch).
func (f *hotSwap) next(t *testing.T, eventType EventType, within time.Duration) Event {
	t.Helper()
	for i, event := range f.pending {
		if event.Type == eventType {
			f.pending = append(f.pending[:i], f.pending[i+1:]...)
			return event
		}
	}
	deadline := time.After(within)
	for {
		select {
		case event := <-f.events:
			if event.Type == eventType {
				return event
			}
			f.pending = append(f.pending, event)
		case <-deadline:
			t.Fatalf("no %s event within %s; log:\n%s", eventType, within, f.logged())
		}
	}
}

// An apply while a download runs (new rules, same nodes) switches kernels
// without touching the download: it completes in full after the switch,
// new connections use the new kernel, and the old kernel drains when the
// download ends.
func TestApplyKeepsRunningConnections(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	username, password := f.user(t, "") // routed user: profile rules, then selected
	tunnel, done := f.startDownload(t, username, password)
	defer tunnel.Close()

	// New revision: a direct rule for the quick target's port.
	rules := []profile.RoutingRule{{ID: "direct-quick", Match: profile.RoutingMatch{Ports: []uint16{targetPort(t, f.quick)}}, Action: profile.RoutingAction{Type: "direct"}}}
	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, rules), time.Now()); err != nil {
		t.Fatal(err)
	}
	switched := f.next(t, EventKernelSwitched, 5*time.Second)
	if switched.ClosedConnections != 0 || switched.KeptConnections < 1 || switched.Revision != "r2" {
		t.Fatalf("switch event: %#v", switched)
	}
	if got := f.core.Status().DrainingKernels; got != 1 {
		t.Fatalf("draining kernels %d, want 1", got)
	}
	if err := f.quickGet(t, username, password); err != nil {
		t.Fatalf("new connection after the switch: %v", err)
	}
	f.releaseAll()
	if err := <-done; err != nil {
		t.Fatalf("download across the switch: %v", err)
	}
	tunnel.Close()
	drained := f.next(t, EventKernelDrained, 5*time.Second)
	if drained.Code != "idle" || drained.ClosedConnections != 0 {
		t.Fatalf("drain event: %#v", drained)
	}
	if got := f.core.Status().DrainingKernels; got != 0 {
		t.Fatalf("draining kernels %d after drain", got)
	}
	if logged := f.logged(); !strings.Contains(logged, `msg="kernel switched"`) || !strings.Contains(logged, `msg="kernel drained" gen=1 reason=idle`) || strings.Contains(logged, "apply full restart") {
		t.Fatalf("log:\n%s", logged)
	}
}

// Taking a node away closes its connections (and its local proxy user's) on
// the switch; a connection on a node that stays keeps running.
func TestApplyClosesConnectionsOfRemovedNodes(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	userA, passwordA := f.user(t, "a")
	userB, passwordB := f.user(t, "b")
	keptTunnel, kept := f.startDownload(t, userA, passwordA)
	defer keptTunnel.Close()
	closedTunnel, closed := f.startDownload(t, userB, passwordB)
	defer closedTunnel.Close()

	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	switched := f.next(t, EventKernelSwitched, 5*time.Second)
	if switched.ClosedConnections != 1 || switched.KeptConnections != 1 {
		t.Fatalf("switch event: %#v", switched)
	}
	select {
	case err := <-closed:
		if err == nil {
			t.Fatal("download on the removed node completed")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("connection on the removed node still open")
	}
	f.releaseAll()
	if err := <-kept; err != nil {
		t.Fatalf("download on the remaining node: %v", err)
	}
	// The removed node's username is gone from the listener too.
	if err := f.quickGet(t, userB, passwordB); err == nil || !strings.Contains(err.Error(), "407") {
		t.Fatalf("removed node user: %v", err)
	}
}

// A new reject rule closes the connections it now matches.
func TestApplyClosesConnectionsANewRuleRejects(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	username, password := f.user(t, "")
	tunnel, done := f.startDownload(t, username, password)
	defer tunnel.Close()
	rules := []profile.RoutingRule{{ID: "block-slow", Match: profile.RoutingMatch{Ports: []uint16{f.slowPort}}, Action: profile.RoutingAction{Type: "reject"}}}
	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, rules), time.Now()); err != nil {
		t.Fatal(err)
	}
	if switched := f.next(t, EventKernelSwitched, 5*time.Second); switched.ClosedConnections != 1 {
		t.Fatalf("switch event: %#v", switched)
	}
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("rejected download completed")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("rejected connection still open")
	}
}

// A kernel that fails to start is discarded: ApplyProfile fails, the old
// kernel and its connections are untouched, and the old profile stays.
func TestApplyKernelStartFailureLeavesTheOldKernel(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	username, password := f.user(t, "")
	tunnel, done := f.startDownload(t, username, password)
	defer tunnel.Close()
	beforeKernelStart = func() error { return errors.New("injected kernel start failure") }
	t.Cleanup(func() { beforeKernelStart = nil })
	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a"}, nil), time.Now()); err == nil || !strings.Contains(err.Error(), "injected") {
		t.Fatalf("apply: %v", err)
	}
	beforeKernelStart = nil
	status := f.core.Status()
	if status.Revision != "r1" || status.DrainingKernels != 0 || status.State != StateRunning {
		t.Fatalf("status after failure: %#v", status)
	}
	if err := f.quickGet(t, username, password); err != nil {
		t.Fatalf("new connection after the failure: %v", err)
	}
	f.releaseAll()
	if err := <-done; err != nil {
		t.Fatalf("download across the failure: %v", err)
	}
}

// Rapid applies stack draining kernels; each drains on its own once its
// connections end, and none is left behind.
func TestRapidAppliesDrainEveryKernel(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	username, password := f.user(t, "")
	tunnel, done := f.startDownload(t, username, password) // held by kernel 1
	defer tunnel.Close()
	for i := 2; i <= 6; i++ {
		if _, err := f.core.ApplyProfile(f.profile("r"+strconv.Itoa(i), []string{"a", "b"}, nil), time.Now()); err != nil {
			t.Fatal(err)
		}
		f.next(t, EventKernelSwitched, 5*time.Second)
		if err := f.quickGet(t, username, password); err != nil {
			t.Fatalf("after apply %d: %v", i, err)
		}
	}
	// Kernels 2..5 had only short connections: they drain while kernel 1
	// still carries the download.
	drained := map[string]bool{}
	for range 4 {
		event := f.next(t, EventKernelDrained, 5*time.Second)
		drained[event.Code] = true
	}
	if got := f.core.Status().DrainingKernels; got != 1 {
		t.Fatalf("draining kernels %d, want only kernel 1", got)
	}
	f.releaseAll()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	tunnel.Close()
	f.next(t, EventKernelDrained, 5*time.Second)
	if got := f.core.Status().DrainingKernels; got != 0 {
		t.Fatalf("draining kernels %d at the end", got)
	}
	if !strings.Contains(f.logged(), `msg="kernel drained" gen=1 reason=idle`) {
		t.Fatalf("kernel 1 not drained:\n%s", f.logged())
	}
}

// A kernel past drainLimit is closed with what it still carries.
func TestDrainDeadlineClosesTheRest(t *testing.T) {
	shortDrain(t, 300*time.Millisecond)
	f := newHotSwap(t)
	username, password := f.user(t, "")
	tunnel, done := f.startDownload(t, username, password)
	defer tunnel.Close()
	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	drained := f.next(t, EventKernelDrained, 5*time.Second)
	if drained.Code != "deadline" || drained.ClosedConnections != 1 {
		t.Fatalf("drain event: %#v", drained)
	}
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("download outlived its kernel")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("connection left open after the drain deadline")
	}
}

// Switching routing_mode is a kernel switch: an existing connection keeps
// running (global only loosens what rules allowed), new ones follow global.
func TestRoutingModeSwitchKeepsConnections(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	rules := []profile.RoutingRule{{ID: "direct-slow", Match: profile.RoutingMatch{Ports: []uint16{f.slowPort}}, Action: profile.RoutingAction{Type: "direct"}}}
	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, rules), time.Now()); err != nil {
		t.Fatal(err)
	}
	f.next(t, EventKernelSwitched, 5*time.Second)
	username, password := f.user(t, "")
	tunnel, done := f.startDownload(t, username, password) // direct by rule
	defer tunnel.Close()
	if _, err := f.core.ApplyProfileWithOptions(f.profile("r2", []string{"a", "b"}, rules), time.Now(), ApplyOptions{RoutingMode: RoutingModeGlobal}); err != nil {
		t.Fatal(err)
	}
	if switched := f.next(t, EventKernelSwitched, 5*time.Second); switched.ClosedConnections != 0 || switched.KeptConnections < 1 {
		t.Fatalf("switch event: %#v", switched)
	}
	if mode := f.core.Status().RoutingMode; mode != RoutingModeGlobal {
		t.Fatalf("mode %q", mode)
	}
	f.releaseAll()
	if err := <-done; err != nil {
		t.Fatalf("download across the mode switch: %v", err)
	}
}

// fullRestartReasons is a whitelist: only listener changes stop the engine.
func TestFullRestartReasons(t *testing.T) {
	listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	localProxy := func(port uint16, password string, users ...string) option.Inbound {
		options := &proxyinbound.Options{ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: port}}
		for _, user := range users {
			options.Users = append(options.Users, proxyinbound.User{Username: user, Password: password})
		}
		return option.Inbound{Type: proxyinbound.Type, Tag: config.LocalProxyInboundTag, Options: options}
	}
	tun := func(mtu uint32) option.Inbound {
		return option.Inbound{Type: C.TypeTun, Tag: config.TUNInboundTag, Options: &option.TunInboundOptions{MTU: mtu, AutoRoute: true}}
	}
	route := &option.RouteOptions{AutoDetectInterface: true}
	running := &layeredEngine{inbounds: []option.Inbound{localProxy(7890, "s", "p-a", "p"), tun(9000)}, route: route}
	for name, c := range map[string]struct {
		inbounds []option.Inbound
		route    *option.RouteOptions
		want     string
	}{
		"same":                   {[]option.Inbound{localProxy(7890, "s", "p-a", "p"), tun(9000)}, route, ""},
		"local proxy users only": {[]option.Inbound{localProxy(7890, "s", "p-b", "p"), tun(9000)}, route, ""},
		"system proxy added":     {[]option.Inbound{localProxy(7890, "s", "p-a", "p"), tun(9000), config.SystemProxyInbound(7891)}, route, ""},
		"rules and nodes":        {[]option.Inbound{localProxy(7890, "s", "p-a", "p"), tun(9000)}, &option.RouteOptions{AutoDetectInterface: true, Final: "other"}, ""},
		"tun options":            {[]option.Inbound{localProxy(7890, "s", "p-a", "p"), tun(1500)}, route, "tun options changed"},
		"local proxy port":       {[]option.Inbound{localProxy(7899, "s", "p-a", "p"), tun(9000)}, route, "local proxy listener changed"},
		"local proxy password":   {[]option.Inbound{localProxy(7890, "x", "p-a", "p"), tun(9000)}, route, ""}, // users, replaced in place
		"tun removed":            {[]option.Inbound{localProxy(7890, "s", "p-a", "p")}, route, "inbound tun added or removed"},
		"interface options":      {[]option.Inbound{localProxy(7890, "s", "p-a", "p"), tun(9000)}, &option.RouteOptions{}, "interface options changed"},
	} {
		got := strings.Join(running.fullRestartReasons(option.Options{Inbounds: c.inbounds, Route: c.route}), "; ")
		if got != c.want {
			t.Errorf("%s: %q, want %q", name, got, c.want)
		}
	}
}

// fakeSwap is an engine that can swap kernels; its full-restart decision is
// the layered engine's real whitelist over the options it runs.
type fakeSwap struct {
	options  option.Options
	swaps    int
	restarts []string
}

func (f *fakeSwap) Start() error { return nil }
func (f *fakeSwap) Close() error { return nil }
func (f *fakeSwap) fullRestartReasons(options option.Options) []string {
	reasons := (&layeredEngine{inbounds: f.options.Inbounds, route: f.options.Route}).fullRestartReasons(options)
	f.restarts = append(f.restarts, strings.Join(reasons, "; "))
	return reasons
}
func (f *fakeSwap) swap(_ context.Context, options option.Options, prepare func(engine), _ func(engine, trackedView) bool) (kernelEvent, error) {
	f.options = options
	f.swaps++
	return kernelEvent{Switched: true}, nil
}
func (f *fakeSwap) drainingKernels() int              { return 0 }
func (f *fakeSwap) setKernelEvents(func(kernelEvent)) {}

// With the host's IPv6 state unchanged, a TUN apply is a kernel switch: the
// no-IPv6-path build (direct wrapped, direct-host resolving IPv4 only, see
// #48) changes outbounds, never the listeners. A change of the IPv6 path
// alone is still a switch; disabling IPv6 changes the TUN and restarts.
func TestTUNApplyWithoutIPv6PathSwitchesKernels(t *testing.T) {
	var engines []*fakeSwap
	core := newCore(profile.PlatformCapabilities{Platform: "windows", TUN: profile.TUNCapabilities{Enabled: true}}, func(_ context.Context, options option.Options) (engine, error) {
		e := &fakeSwap{options: options}
		engines = append(engines, e)
		return e, nil
	})
	core.hostIPv6 = func() bool { return true }
	route := false
	core.hostIPv6Route = func() (bool, error) { return route, nil }
	if _, err := core.ApplyProfile(testProfile("r1", "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	for _, revision := range []string{"r2", "r3"} {
		if _, err := core.ApplyProfile(testProfile(revision, "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if len(engines) != 1 || engines[0].swaps != 2 || strings.Join(engines[0].restarts, "") != "" {
		t.Fatalf("engines %d, swaps %d, restart reasons %q", len(engines), engines[0].swaps, engines[0].restarts)
	}
	if !core.built.DirectIPv6HandOff {
		t.Fatal("build is not the no-IPv6-path one: the test does not cover direct-host")
	}
	// The host gains an IPv6 path: outbounds only, still a switch.
	route = true
	if _, err := core.ApplyProfile(testProfile("r4", "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if len(engines) != 1 || engines[0].swaps != 3 || core.built.DirectIPv6HandOff {
		t.Fatalf("after the IPv6 path appeared: engines %d swaps %d handoff %v", len(engines), engines[0].swaps, core.built.DirectIPv6HandOff)
	}
	// IPv6 disabled on the host: the TUN loses its IPv6 address, restart.
	core.hostIPv6 = func() bool { return false }
	if _, err := core.ApplyProfile(testProfile("r5", "edge.example.com", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if len(engines) != 2 || engines[0].restarts[len(engines[0].restarts)-1] != "tun options changed" {
		t.Fatalf("after IPv6 was disabled: engines %d, last reasons %q", len(engines), engines[0].restarts)
	}
}

// An apply in progress must not wedge the core's lock: Status reads the
// kernel while holding it (activeIngress), and the apply's prepare (pins)
// takes it. With a writer queued on the core's lock in between (Subscribe
// here; a failover switch event or SetSystemProxy in practice), reading the
// kernel under the engine's own lock made a cycle: apply waits for the
// writer, the writer for Status, Status for the engine lock apply holds.
func TestApplyDoesNotDeadlockWithStatusAndAWriter(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	paused, release := make(chan struct{}), make(chan struct{})
	beforeKernelStart = func() error { close(paused); <-release; return nil }
	t.Cleanup(func() { beforeKernelStart = nil })
	applied := make(chan error, 1)
	go func() {
		_, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, nil), time.Now())
		applied <- err
	}()
	<-paused
	status := make(chan Status, 1)
	go func() { status <- f.core.Status() }()
	time.Sleep(100 * time.Millisecond) // Status holds the core's read lock
	subscribed := make(chan struct{})
	go func() {
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		f.core.Subscribe(ctx, 1) // queues for the core's write lock
		close(subscribed)
	}()
	time.Sleep(100 * time.Millisecond)
	close(release) // the apply goes on to prepare, which takes the read lock
	timeout := time.After(5 * time.Second)
	for name, done := range map[string]<-chan struct{}{"status": waitFor(status), "subscribe": subscribed, "apply": waitFor(applied)} {
		select {
		case <-done:
		case <-timeout:
			t.Fatalf("%s did not return: lock cycle", name)
		}
	}
}

func waitFor[T any](ch <-chan T) <-chan struct{} {
	done := make(chan struct{})
	go func() { <-ch; close(done) }()
	return done
}

// shortDrainIdle is shortDrain with idle connections of draining kernels
// closed after idle.
func shortDrainIdle(t *testing.T, idle time.Duration) {
	t.Helper()
	shortDrain(t, time.Minute)
	previous := drainIdleClose
	drainIdleClose = idle
	t.Cleanup(func() { drainIdleClose = previous })
}

// keepAlive opens a CONNECT tunnel as the routed user to the quick target
// and makes one request on it; the tunnel stays open for more.
func (f *hotSwap) keepAlive(t *testing.T) (net.Conn, func() error) {
	t.Helper()
	username, password := f.user(t, "")
	endpoint := f.core.LocalProxyEndpoints()[0]
	tunnel, status, err := httpConnect(endpoint, username, password, f.quick.Listener.Addr().String())
	if err != nil || status[:3] != "200" {
		t.Fatalf("CONNECT: %q %v", status, err)
	}
	_ = tunnel.SetDeadline(time.Now().Add(30 * time.Second))
	request := func() error {
		code, err := tunnelGet(tunnel, f.quick.Listener.Addr().String())
		if err == nil && code != http.StatusNoContent {
			err = errors.New(strconv.Itoa(code))
		}
		return err
	}
	if err = request(); err != nil {
		t.Fatal(err)
	}
	return tunnel, request
}

// A keep-alive connection that goes quiet in a draining kernel is closed
// after drainIdleClose, and the kernel drains instead of waiting for
// drainLimit.
func TestDrainClosesIdleConnections(t *testing.T) {
	shortDrainIdle(t, 500*time.Millisecond)
	f := newHotSwap(t)
	tunnel, _ := f.keepAlive(t)
	defer tunnel.Close()
	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	if switched := f.next(t, EventKernelSwitched, 5*time.Second); switched.KeptConnections != 1 {
		t.Fatalf("switch event: %#v", switched)
	}
	drained := f.next(t, EventKernelDrained, 5*time.Second)
	if drained.Code != "idle" {
		t.Fatalf("drain event: %#v", drained)
	}
	if !strings.Contains(f.logged(), `msg="kernel drained" gen=1 reason=idle closed_connections=0 idle_closed=1`) {
		t.Fatalf("log:\n%s", f.logged())
	}
	_ = tunnel.SetReadDeadline(time.Now().Add(2 * time.Second))
	if _, err := tunnel.Read(make([]byte, 1)); err == nil || errors.Is(err, os.ErrDeadlineExceeded) {
		t.Fatalf("idle connection still open: %v", err)
	}
}

// A connection that keeps moving bytes is not idle, however long it runs in
// a draining kernel; the kernel drains once it ends.
func TestDrainKeepsActiveConnections(t *testing.T) {
	shortDrainIdle(t, 500*time.Millisecond)
	f := newHotSwap(t)
	trickle := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Length", strconv.Itoa(20*1024))
		for range 20 {
			if _, err := w.Write(make([]byte, 1024)); err != nil {
				return
			}
			w.(http.Flusher).Flush()
			time.Sleep(100 * time.Millisecond) // well under the idle threshold
		}
	}))
	defer trickle.Close()
	username, password := f.user(t, "")
	endpoint := f.core.LocalProxyEndpoints()[0]
	tunnel, status, err := httpConnect(endpoint, username, password, trickle.Listener.Addr().String())
	if err != nil || status[:3] != "200" {
		t.Fatalf("CONNECT: %q %v", status, err)
	}
	defer tunnel.Close()
	_ = tunnel.SetDeadline(time.Now().Add(30 * time.Second))
	if _, err = io.WriteString(tunnel, "GET / HTTP/1.1\r\nHost: trickle\r\n\r\n"); err != nil {
		t.Fatal(err)
	}
	response, err := http.ReadResponse(bufio.NewReader(tunnel), nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	f.next(t, EventKernelSwitched, 5*time.Second)
	if n, err := io.Copy(io.Discard, response.Body); err != nil || n != 20*1024 {
		t.Fatalf("2 s trickle across the drain: %d %v", n, err)
	}
	tunnel.Close()
	f.next(t, EventKernelDrained, 5*time.Second)
	if !strings.Contains(f.logged(), `msg="kernel drained" gen=1 reason=idle closed_connections=0 idle_closed=0`) {
		t.Fatalf("log:\n%s", f.logged())
	}
}

// Pauses shorter than the threshold do not close a draining connection.
func TestDrainToleratesShortPauses(t *testing.T) {
	shortDrainIdle(t, 600*time.Millisecond)
	f := newHotSwap(t)
	tunnel, request := f.keepAlive(t)
	defer tunnel.Close()
	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	f.next(t, EventKernelSwitched, 5*time.Second)
	for i := range 2 {
		time.Sleep(300 * time.Millisecond)
		if err := request(); err != nil {
			t.Fatalf("request %d after a short pause: %v", i, err)
		}
	}
	if got := f.core.Status().DrainingKernels; got != 1 {
		t.Fatalf("draining kernels %d: the connection was closed", got)
	}
}

// A switch applies the new profile to every replaced kernel still draining,
// not only the one it replaces: a download started two applies earlier is
// counted as kept, and closed once its node is removed.
func TestApplyReachesConnectionsOfOlderKernels(t *testing.T) {
	shortDrain(t, time.Minute)
	f := newHotSwap(t)
	userA, passwordA := f.user(t, "a")
	userB, passwordB := f.user(t, "b")
	keptTunnel, kept := f.startDownload(t, userA, passwordA) // kernel 1
	defer keptTunnel.Close()
	closedTunnel, closed := f.startDownload(t, userB, passwordB) // kernel 1
	defer closedTunnel.Close()

	if _, err := f.core.ApplyProfile(f.profile("r2", []string{"a", "b"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	// Each connection carries the node its kernel routed it to.
	layered := f.core.engine.(*layeredEngine)
	recorded := map[string]bool{}
	for _, item := range layered.tracker.generation(1) {
		recorded[item.nodeID] = true
	}
	if !recorded["a"] || !recorded["b"] {
		t.Fatalf("node IDs recorded by kernel 1: %v", recorded)
	}

	if switched := f.next(t, EventKernelSwitched, 5*time.Second); switched.KeptConnections != 2 || switched.ClosedConnections != 0 || switched.DrainingKernels != 1 {
		t.Fatalf("first switch: %#v", switched)
	}
	// Kernel 2 carried nothing; kernel 1 still carries both downloads.
	if _, err := f.core.ApplyProfile(f.profile("r3", []string{"a"}, nil), time.Now()); err != nil {
		t.Fatal(err)
	}
	switched := f.next(t, EventKernelSwitched, 5*time.Second)
	if switched.KeptConnections != 1 || switched.ClosedConnections != 1 || switched.DrainingKernels != 2 {
		t.Fatalf("second switch (kernel 1's connections must count): %#v", switched)
	}
	select {
	case err := <-closed:
		if err == nil {
			t.Fatal("download on the removed node completed")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("kernel 1's connection on the removed node still open")
	}
	f.releaseAll()
	if err := <-kept; err != nil {
		t.Fatalf("download on the remaining node: %v", err)
	}
	if logged := f.logged(); !strings.Contains(logged, `msg="kernel switched" gen=3 previous=2 closed_connections=1 kept_connections=1 draining_kernels=2`) {
		t.Fatalf("log:\n%s", logged)
	}
}
