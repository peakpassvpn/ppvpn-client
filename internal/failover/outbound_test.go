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
	g.recoverInterval = 10 * time.Millisecond
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

func TestHealthLoopIsLazyAndChecksOnlyPrimaryWhenHealthy(t *testing.T) {
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
	mu.Lock()
	defer mu.Unlock()
	if checked["b"] != 0 {
		t.Fatalf("backup checked while primary healthy: %v", checked)
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
	if _, err := New(context.Background(), nil, log.NewNOPFactory().NewLogger("test"), "node", Options{Outbounds: []string{"a"}, URL: "https://example.com"}); err == nil {
		t.Fatal("https health URL accepted")
	}
}
