package failover

import (
	"context"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing-box/log"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
)

type fakeOutbound struct {
	tag     string
	network []string
	fail    atomic.Bool
	dials   atomic.Int32
}

func (f *fakeOutbound) Type() string           { return "fake" }
func (f *fakeOutbound) Tag() string            { return f.tag }
func (f *fakeOutbound) Network() []string      { return f.network }
func (f *fakeOutbound) Dependencies() []string { return nil }
func (f *fakeOutbound) DialContext(ctx context.Context, _ string, _ M.Socksaddr) (net.Conn, error) {
	f.dials.Add(1)
	if f.fail.Load() {
		return nil, errors.New("connection refused")
	}
	a, b := net.Pipe()
	go func() { <-ctx.Done(); b.Close() }()
	return a, nil
}
func (f *fakeOutbound) ListenPacket(context.Context, M.Socksaddr) (net.PacketConn, error) {
	f.dials.Add(1)
	if f.fail.Load() {
		return nil, errors.New("unreachable")
	}
	return net.ListenPacket("udp", "127.0.0.1:0")
}

func newTestGroup(t *testing.T, members ...*fakeOutbound) *Group {
	t.Helper()
	tags := make([]string, len(members))
	for i, m := range members {
		tags[i] = m.tag
	}
	out, err := New(context.Background(), nil, log.NewNOPFactory().NewLogger("test"), "node", Options{Outbounds: tags})
	if err != nil {
		t.Fatal(err)
	}
	g := out.(*Group)
	for _, m := range members {
		member := &member{outbound: m}
		member.healthy.Store(true)
		g.members = append(g.members, member)
	}
	g.check = func(_ context.Context, o adapter.Outbound) error {
		if o.(*fakeOutbound).fail.Load() {
			return errors.New("check failed")
		}
		return nil
	}
	t.Cleanup(func() { _ = g.Close() })
	return g
}

func dial(t *testing.T, g *Group) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	conn, err := g.DialContext(ctx, N.NetworkTCP, M.ParseSocksaddr("example.com:443"))
	if err != nil {
		t.Fatal(err)
	}
	conn.Close()
}

func TestRejectsInvalidMembers(t *testing.T) {
	for _, tags := range [][]string{nil, {""}, {"a", "a"}, {"node"}} {
		if _, err := New(context.Background(), nil, log.NewNOPFactory().NewLogger("test"), "node", Options{Outbounds: tags}); err == nil {
			t.Fatalf("accepted %v", tags)
		}
	}
}

func TestPrefersPrimaryAndFailsOverImmediately(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP, N.NetworkUDP}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP, N.NetworkUDP}}
	g := newTestGroup(t, primary, backup)
	dial(t, g)
	if primary.dials.Load() != 1 || backup.dials.Load() != 0 || g.Now() != "p" {
		t.Fatalf("primary not preferred: p=%d b=%d now=%s", primary.dials.Load(), backup.dials.Load(), g.Now())
	}
	primary.fail.Store(true)
	dial(t, g) // primary fails, backup serves the same dial
	if backup.dials.Load() != 1 || g.Now() != "b" {
		t.Fatalf("no failover: b=%d now=%s", backup.dials.Load(), g.Now())
	}
	before := primary.dials.Load()
	dial(t, g) // unhealthy primary is skipped
	if primary.dials.Load() != before || backup.dials.Load() != 2 {
		t.Fatal("unhealthy primary was retried before recovery")
	}
}

func TestReturnsToPrimaryAfterRecovery(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP}}
	g := newTestGroup(t, primary, backup)
	g.interval, g.minDwell = 10*time.Millisecond, 0
	g.Start(adapter.StartStateStarted)
	primary.fail.Store(true)
	dial(t, g)
	if g.Now() != "b" {
		t.Fatalf("now=%s", g.Now())
	}
	primary.fail.Store(false)
	deadline := time.Now().Add(2 * time.Second)
	for g.Now() != "p" {
		if time.Now().After(deadline) {
			t.Fatal("did not return to primary")
		}
		time.Sleep(5 * time.Millisecond)
	}
	dial(t, g)
	if backup.dials.Load() != 1 {
		t.Fatalf("backup used after recovery: %d", backup.dials.Load())
	}
}

func TestHealthLoopIsLazyAndChecksEveryMember(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP}}
	g := newTestGroup(t, primary, backup)
	var mu sync.Mutex
	checked := map[string]int{}
	g.check = func(_ context.Context, o adapter.Outbound) error {
		mu.Lock()
		checked[o.Tag()]++
		mu.Unlock()
		return nil
	}
	g.Start(adapter.StartStateStarted)
	time.Sleep(20 * time.Millisecond)
	mu.Lock()
	if len(checked) != 0 {
		t.Fatalf("idle group generated health checks: %v", checked)
	}
	mu.Unlock()
	dial(t, g)
	deadline := time.Now().Add(time.Second)
	for {
		mu.Lock()
		n := checked["p"]
		mu.Unlock()
		if n > 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("primary never checked")
		}
		time.Sleep(5 * time.Millisecond)
	}
	for {
		mu.Lock()
		n := checked["b"]
		mu.Unlock()
		if n > 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("backup never checked")
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func TestNetworkFilteringAndAllFailed(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP, N.NetworkUDP}}
	g := newTestGroup(t, primary, backup)
	conn, err := g.ListenPacket(context.Background(), M.ParseSocksaddr("1.1.1.1:53"))
	if err != nil {
		t.Fatal(err)
	}
	conn.Close()
	if primary.dials.Load() != 0 || backup.dials.Load() != 1 {
		t.Fatal("UDP routed to TCP-only primary")
	}
	primary.fail.Store(true)
	backup.fail.Store(true)
	if _, err = g.DialContext(context.Background(), N.NetworkTCP, M.ParseSocksaddr("example.com:443")); err == nil {
		t.Fatal("dial succeeded with every ingress down")
	}
	// Everything unhealthy: the group still tries members in order.
	primary.fail.Store(false)
	dial(t, g)
}

type directOutbound struct{ fakeOutbound }

func (d *directOutbound) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	return (&net.Dialer{}).DialContext(ctx, network, destination.String())
}

func TestHTTPCheck(t *testing.T) {
	status := http.StatusNoContent
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodHead || r.URL.Path != "/generate_204" {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		w.WriteHeader(status)
	}))
	defer server.Close()
	target, _ := url.Parse(server.URL + "/generate_204")
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	member := &directOutbound{fakeOutbound{tag: "d", network: []string{N.NetworkTCP}}}
	if err := httpCheck(ctx, target, member); err != nil {
		t.Fatal(err)
	}
	status = http.StatusBadGateway
	if err := httpCheck(ctx, target, member); err == nil {
		t.Fatal("5xx accepted")
	}
	if _, err := New(context.Background(), nil, log.NewNOPFactory().NewLogger("test"), "node", Options{Outbounds: []string{"a"}, URLs: []string{"https://example.com"}}); err == nil {
		t.Fatal("https health URL accepted")
	}
}

func TestActiveTracksSwitchesAndNotifiesObserver(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP, N.NetworkUDP}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP, N.NetworkUDP}}
	var mu sync.Mutex
	var switches []Active
	ctx := WithSwitchObserver(context.Background(), func(group string, active Active) {
		if group != "node" {
			t.Errorf("group %q", group)
		}
		mu.Lock()
		switches = append(switches, active)
		mu.Unlock()
	})
	out, err := New(ctx, nil, log.NewNOPFactory().NewLogger("test"), "node", Options{Outbounds: []string{"p", "b"}})
	if err != nil {
		t.Fatal(err)
	}
	g := out.(*Group)
	g.minDwell = 0 // switch back as soon as the primary is healthy
	for _, m := range []*fakeOutbound{primary, backup} {
		member := &member{outbound: m}
		member.healthy.Store(true)
		g.members = append(g.members, member)
	}
	t.Cleanup(func() { _ = g.Close() })

	if active := g.Active(); active.Current != "p" || active.Previous != "" || !active.SwitchedAt.IsZero() {
		t.Fatalf("initial: %#v", active)
	}
	dial(t, g)
	if active := g.Active(); active.Current != "p" || active.Previous != "" || len(switches) != 0 {
		t.Fatalf("primary use is not a switch: %#v %v", active, switches)
	}
	primary.fail.Store(true)
	before := time.Now()
	dial(t, g)
	failedOver := g.Active()
	if failedOver.Current != "b" || failedOver.Previous != "p" || failedOver.SwitchedAt.Before(before) {
		t.Fatalf("after failover: %#v", failedOver)
	}
	dial(t, g)
	if g.Active() != failedOver {
		t.Fatal("repeated use of the same member recorded a switch")
	}
	// Recovery: the next connection returns to the primary.
	primary.fail.Store(false)
	g.members[0].healthy.Store(true)
	ctxUDP, cancel := context.WithCancel(context.Background())
	defer cancel()
	packet, err := g.ListenPacket(ctxUDP, M.ParseSocksaddr("1.1.1.1:53"))
	if err != nil {
		t.Fatal(err)
	}
	packet.Close()
	if active := g.Active(); active.Current != "p" || active.Previous != "b" {
		t.Fatalf("after recovery: %#v", active)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(switches) != 2 || switches[0] != failedOver || switches[1].Current != "p" {
		t.Fatalf("observer saw %#v", switches)
	}
}

// Checks mark a member unhealthy only after UnhealthyAfter consecutive
// failures, and healthy again only after RecoverAfter consecutive passes; a
// failed dial marks it unhealthy at once.
func TestProbeThresholds(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP}}
	g := newTestGroup(t, primary)
	m := g.members[0]
	primary.fail.Store(true)
	for i := 1; i < UnhealthyAfter; i++ {
		g.probe(m)
		if !m.healthy.Load() {
			t.Fatalf("unhealthy after %d failed check(s)", i)
		}
	}
	g.probe(m)
	if m.healthy.Load() || g.Members()[0].ConsecutiveFailures != UnhealthyAfter || g.Members()[0].LastCheck.IsZero() {
		t.Fatalf("after %d failures: %+v", UnhealthyAfter, g.Members()[0])
	}
	primary.fail.Store(false)
	for i := 1; i < RecoverAfter; i++ {
		g.probe(m)
		if m.healthy.Load() {
			t.Fatalf("recovered after %d pass(es)", i)
		}
	}
	g.probe(m)
	if !m.healthy.Load() || g.Members()[0].ConsecutiveFailures != 0 {
		t.Fatalf("not recovered: %+v", g.Members()[0])
	}
	// One failed check in between resets the pass count.
	g.markUnhealthy(m, errors.New("dial failed"))
	g.probe(m)
	g.probe(m)
	primary.fail.Store(true)
	g.probe(m)
	primary.fail.Store(false)
	g.probe(m)
	g.probe(m)
	if m.healthy.Load() {
		t.Fatal("recovered without RecoverAfter consecutive passes")
	}
}

// Within minDwell of a switch the backup keeps leading even once the primary
// is healthy again; afterwards the primary leads.
func TestDwellKeepsBackupAfterSwitch(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP}}
	g := newTestGroup(t, primary, backup)
	g.minDwell = time.Hour
	primary.fail.Store(true)
	dial(t, g)
	if g.Now() != "b" {
		t.Fatalf("now=%s", g.Now())
	}
	primary.fail.Store(false)
	g.members[0].healthy.Store(true)
	if g.Now() != "b" {
		t.Fatal("switched back to the primary within the dwell")
	}
	// An explicit unpin is a choice, not a flap: it clears the dwell.
	if err := g.Pin(""); err != nil || g.Now() != "p" {
		t.Fatalf("unpin kept the dwell: %v %s", err, g.Now())
	}
	dial(t, g) // the primary carries traffic again
	primary.fail.Store(true)
	dial(t, g)
	primary.fail.Store(false)
	g.members[0].healthy.Store(true)
	if g.Now() != "b" {
		t.Fatal("automatic switch did not start a dwell")
	}
	g.minDwell = 0
	if g.Now() != "p" {
		t.Fatalf("primary not preferred after the dwell: %s", g.Now())
	}
}

// A pinned member carries every connection alone, with no fallback even
// while it fails; unpinning restores automatic selection. Unknown tags are
// rejected, and a pin set before Start applies once members exist.
func TestPinnedMemberHasNoFallback(t *testing.T) {
	primary := &fakeOutbound{tag: "p", network: []string{N.NetworkTCP}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP}}
	g := newTestGroup(t, primary, backup)
	if err := g.Pin("x"); !errors.Is(err, ErrUnknownMember) {
		t.Fatalf("unknown tag: %v", err)
	}
	if err := g.Pin("b"); err != nil || g.Pinned() != "b" || g.Now() != "b" {
		t.Fatalf("pin: %v %q %q", err, g.Pinned(), g.Now())
	}
	dial(t, g)
	if primary.dials.Load() != 0 || backup.dials.Load() != 1 {
		t.Fatal("pinned dial used another member")
	}
	backup.fail.Store(true)
	if _, err := g.DialContext(context.Background(), N.NetworkTCP, M.ParseSocksaddr("example.com:443")); err == nil || primary.dials.Load() != 0 {
		t.Fatalf("pinned failure fell back: %v, primary dials %d", err, primary.dials.Load())
	}
	if err := g.Pin(""); err != nil || g.Pinned() != "" {
		t.Fatal("unpin")
	}
	dial(t, g)
	if primary.dials.Load() != 1 {
		t.Fatal("automatic selection not restored")
	}

	out, err := New(context.Background(), nil, log.NewNOPFactory().NewLogger("test"), "early", Options{Outbounds: []string{"p", "b"}})
	if err != nil {
		t.Fatal(err)
	}
	early := out.(*Group)
	if err = early.Pin("b"); err != nil || early.Pinned() != "b" {
		t.Fatalf("pin before start: %v %q", err, early.Pinned())
	}
}

type slowOutbound struct{ fakeOutbound }

func (s *slowOutbound) DialContext(ctx context.Context, _ string, _ M.Socksaddr) (net.Conn, error) {
	s.dials.Add(1)
	<-ctx.Done()
	return nil, ctx.Err()
}

// A member that hangs costs dialTimeout, then the next member serves the
// same dial.
func TestDialTimeoutMovesToTheNextMember(t *testing.T) {
	primary := &slowOutbound{fakeOutbound{tag: "p", network: []string{N.NetworkTCP}}}
	backup := &fakeOutbound{tag: "b", network: []string{N.NetworkTCP}}
	out, err := New(context.Background(), nil, log.NewNOPFactory().NewLogger("test"), "node", Options{Outbounds: []string{"p", "b"}})
	if err != nil {
		t.Fatal(err)
	}
	g := out.(*Group)
	g.dialTimeout = 50 * time.Millisecond
	for _, o := range []adapter.Outbound{primary, backup} {
		m := &member{outbound: o}
		m.healthy.Store(true)
		g.members = append(g.members, m)
	}
	t.Cleanup(func() { _ = g.Close() })
	started := time.Now()
	dial(t, g)
	if elapsed := time.Since(started); elapsed > time.Second || backup.dials.Load() != 1 || g.members[0].healthy.Load() {
		t.Fatalf("elapsed %s, backup dials %d", elapsed, backup.dials.Load())
	}
}

// checkAny passes when a later URL answers although the first does not.
func TestCheckFallsBackToTheSecondURL(t *testing.T) {
	down := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusBadGateway) }))
	defer down.Close()
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }))
	defer up.Close()
	first, _ := url.Parse(down.URL + "/generate_204")
	second, _ := url.Parse(up.URL + "/generate_204")
	member := &directOutbound{fakeOutbound{tag: "d", network: []string{N.NetworkTCP}}}
	if err := checkAny(context.Background(), []*url.URL{first, second}, member); err != nil {
		t.Fatal(err)
	}
	if err := checkAny(context.Background(), []*url.URL{first}, member); err == nil {
		t.Fatal("failing URL passed")
	}
	if CheckURLs[0] != "http://www.gstatic.com/generate_204" || CheckURLs[1] != "http://cp.cloudflare.com/generate_204" {
		t.Fatalf("check URLs %v", CheckURLs)
	}
}
