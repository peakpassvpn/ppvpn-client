package dnstransport

import (
	"context"
	"errors"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	mDNS "github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/log"
	"github.com/sagernet/sing/service"
)

// fakeTransport answers each exchange with the next scripted step; a nil step
// blocks until the context ends, like a query sent on a half-open connection.
type fakeTransport struct {
	adapter.DNSTransport
	tag    string
	mu     sync.Mutex
	steps  []func(*mDNS.Msg) (*mDNS.Msg, error)
	calls  int
	resets atomic.Int32
}

func (f *fakeTransport) Tag() string  { return f.tag }
func (f *fakeTransport) Type() string { return C.DNSTypeTLS }
func (f *fakeTransport) Reset()       { f.resets.Add(1) }
func (f *fakeTransport) Exchange(ctx context.Context, query *mDNS.Msg) (*mDNS.Msg, error) {
	f.mu.Lock()
	var step func(*mDNS.Msg) (*mDNS.Msg, error)
	if f.calls < len(f.steps) {
		step = f.steps[f.calls]
	}
	f.calls++
	f.mu.Unlock()
	if step == nil {
		<-ctx.Done()
		return nil, errors.New("read response: i/o timeout")
	}
	return step(query)
}

func answer(rcode int) func(*mDNS.Msg) (*mDNS.Msg, error) {
	return func(query *mDNS.Msg) (*mDNS.Msg, error) {
		response := new(mDNS.Msg)
		response.SetRcode(query, rcode)
		return response, nil
	}
}

func fail(err error) func(*mDNS.Msg) (*mDNS.Msg, error) {
	return func(*mDNS.Msg) (*mDNS.Msg, error) { return nil, err }
}

func construct(t *testing.T, ctx context.Context, inner *fakeTransport) adapter.DNSTransport {
	t.Helper()
	got, err := wrap(func(context.Context, log.ContextLogger, string, struct{}) (adapter.DNSTransport, error) {
		return inner, nil
	})(ctx, nil, inner.tag, struct{}{})
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func query() *mDNS.Msg {
	q := new(mDNS.Msg)
	q.SetQuestion("example.com.", mDNS.TypeA)
	return q
}

// shortGuard shrinks the guard limits for the test.
func shortGuard(t *testing.T) {
	t.Helper()
	a, b, n, i := attemptTimeout, overallBudget, maxAttempts, idleReset
	attemptTimeout, overallBudget, maxAttempts, idleReset = 50*time.Millisecond, 400*time.Millisecond, 3, time.Hour
	t.Cleanup(func() { attemptTimeout, overallBudget, maxAttempts, idleReset = a, b, n, i })
}

func debugLogger() (*corelog.Logger, *strings.Builder) {
	var b strings.Builder
	var mu sync.Mutex
	l := corelog.New(writerFunc(func(p []byte) (int, error) { mu.Lock(); defer mu.Unlock(); return b.Write(p) }))
	_ = l.SetLevel(corelog.LevelDebug)
	return l, &b
}

type writerFunc func([]byte) (int, error)

func (f writerFunc) Write(p []byte) (int, error) { return f(p) }

func TestUnguardedExchangeIsLoggedOnlyAtDebugLevel(t *testing.T) {
	var b strings.Builder
	l := corelog.New(&b)
	inner := &fakeTransport{tag: "dns-local", steps: []func(*mDNS.Msg) (*mDNS.Msg, error){answer(mDNS.RcodeNameError), answer(mDNS.RcodeNameError), fail(errors.New("i/o timeout"))}}
	transport := construct(t, WithLogger(context.Background(), l), inner)
	if transport.Type() != C.DNSTypeTLS {
		t.Fatalf("type not forwarded: %q", transport.Type())
	}
	_, _ = transport.Exchange(context.Background(), query())
	if b.Len() != 0 {
		t.Fatalf("logged at info level: %q", b.String())
	}
	_ = l.SetLevel(corelog.LevelDebug)
	_, _ = transport.Exchange(context.Background(), query())
	if !strings.Contains(b.String(), "msg=dns name=example.com. type=A server=dns-local rcode=NXDOMAIN answers=0 ms=") {
		t.Fatalf("line: %q", b.String())
	}
	b.Reset()
	if _, err := transport.Exchange(context.Background(), query()); err == nil || inner.calls != 3 {
		t.Fatalf("unguarded transport retried or hid the error: %v, calls %d", err, inner.calls)
	}
	if !strings.Contains(b.String(), `server=dns-local error="i/o timeout" ms=`) {
		t.Fatalf("error line: %q", b.String())
	}
}

// Without a logger only the guarded server is wrapped.
func TestWrappingWithoutLogger(t *testing.T) {
	plain := &fakeTransport{tag: "dns-local"}
	if got := construct(t, context.Background(), plain); got != adapter.DNSTransport(plain) {
		t.Fatalf("dns-local wrapped without a logger: %T", got)
	}
	remote := &fakeTransport{tag: GuardedTag}
	if _, ok := construct(t, context.Background(), remote).(*wrapped); !ok {
		t.Fatal("dns-remote not guarded without a logger")
	}
}

// A query swallowed by a half-open connection costs one attempt timeout, not
// the whole DNS timeout: the next attempt answers.
func TestGuardRetriesAfterAHungAttempt(t *testing.T) {
	shortGuard(t)
	l, b := debugLogger()
	inner := &fakeTransport{tag: GuardedTag, steps: []func(*mDNS.Msg) (*mDNS.Msg, error){nil, answer(mDNS.RcodeSuccess)}}
	transport := construct(t, WithLogger(context.Background(), l), inner)
	started := time.Now()
	response, err := transport.Exchange(context.Background(), query())
	elapsed := time.Since(started)
	if err != nil || response == nil || inner.calls != 2 {
		t.Fatalf("response %v, err %v, calls %d", response, err, inner.calls)
	}
	if elapsed < attemptTimeout || elapsed > attemptTimeout+200*time.Millisecond {
		t.Fatalf("took %s", elapsed)
	}
	lines := b.String()
	if !strings.Contains(lines, "server=dns-remote attempt=1 error=") || !strings.Contains(lines, "server=dns-remote attempt=2 rcode=NOERROR") {
		t.Fatalf("lines:\n%s", lines)
	}
}

// EOF (a closed pooled connection) takes the same retry path.
func TestGuardRetriesEOF(t *testing.T) {
	shortGuard(t)
	inner := &fakeTransport{tag: GuardedTag, steps: []func(*mDNS.Msg) (*mDNS.Msg, error){fail(errors.New("read response: EOF")), answer(mDNS.RcodeSuccess)}}
	if _, err := construct(t, context.Background(), inner).Exchange(context.Background(), query()); err != nil || inner.calls != 2 {
		t.Fatalf("err %v, calls %d", err, inner.calls)
	}
}

// When every attempt fails the guard gives up after maxAttempts within the
// overall budget and returns the last error.
func TestGuardGivesUpWithinBudget(t *testing.T) {
	shortGuard(t)
	inner := &fakeTransport{tag: GuardedTag}
	started := time.Now()
	_, err := construct(t, context.Background(), inner).Exchange(context.Background(), query())
	elapsed := time.Since(started)
	if err == nil || inner.calls != maxAttempts {
		t.Fatalf("err %v, calls %d", err, inner.calls)
	}
	if elapsed > overallBudget+100*time.Millisecond {
		t.Fatalf("took %s, budget %s", elapsed, overallBudget)
	}
	// A cancelled caller is not retried.
	inner = &fakeTransport{tag: GuardedTag}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err = construct(t, context.Background(), inner).Exchange(ctx, query()); err == nil || inner.calls != 1 {
		t.Fatalf("cancelled: err %v, calls %d", err, inner.calls)
	}
}

// A DNS answer, even NXDOMAIN or SERVFAIL, is the server's answer: no retry.
func TestGuardDoesNotRetryAnswers(t *testing.T) {
	shortGuard(t)
	inner := &fakeTransport{tag: GuardedTag, steps: []func(*mDNS.Msg) (*mDNS.Msg, error){answer(mDNS.RcodeServerFailure)}}
	response, err := construct(t, context.Background(), inner).Exchange(context.Background(), query())
	if err != nil || response.Rcode != mDNS.RcodeServerFailure || inner.calls != 1 {
		t.Fatalf("response %v, err %v, calls %d", response, err, inner.calls)
	}
}

// After idleReset without a success, and with nothing in flight, the pool is
// reset before the next query; not after a recent success or while another
// query is in flight.
func TestGuardResetsThePoolAfterIdle(t *testing.T) {
	shortGuard(t)
	ok := answer(mDNS.RcodeSuccess)
	inner := &fakeTransport{tag: GuardedTag, steps: []func(*mDNS.Msg) (*mDNS.Msg, error){ok, ok, ok, ok}}
	transport := construct(t, context.Background(), inner).(*wrapped)
	exchange := func() {
		t.Helper()
		if _, err := transport.Exchange(context.Background(), query()); err != nil {
			t.Fatal(err)
		}
	}
	exchange() // first query: nothing to reset
	exchange() // recent success
	if inner.resets.Load() != 0 {
		t.Fatalf("reset without idle: %d", inner.resets.Load())
	}
	transport.lastSuccess.Store(time.Now().Add(-2 * idleReset).UnixNano())
	transport.inflight.Add(1)
	exchange() // idle, but another query is in flight
	transport.inflight.Add(-1)
	if inner.resets.Load() != 0 {
		t.Fatal("reset while another query was in flight")
	}
	transport.lastSuccess.Store(time.Now().Add(-2 * idleReset).UnixNano())
	exchange()
	if inner.resets.Load() != 1 {
		t.Fatalf("resets after idle: %d", inner.resets.Load())
	}
}

// fakeManager resolves the fallback servers by tag.
type fakeManager struct {
	adapter.DNSTransportManager
	transports map[string]adapter.DNSTransport
}

func (m *fakeManager) Transport(tag string) (adapter.DNSTransport, bool) {
	transport, ok := m.transports[tag]
	return transport, ok
}

// guardWithFallbacks builds dns-remote and its FallbackTags servers the way
// sing-box does: every server through wrap, the manager in the context.
func guardWithFallbacks(t *testing.T, l *corelog.Logger, primary *fakeTransport, fallbacks ...*fakeTransport) *wrapped {
	t.Helper()
	manager := &fakeManager{transports: map[string]adapter.DNSTransport{}}
	ctx := service.ContextWith[adapter.DNSTransportManager](WithLogger(context.Background(), l), manager)
	guarded := construct(t, ctx, primary).(*wrapped)
	for _, fallback := range fallbacks {
		manager.transports[fallback.tag] = construct(t, ctx, fallback)
	}
	return guarded
}

func countLines(lines, substring string) int { return strings.Count(lines, substring) }

// Each attempt goes to the next upstream in order: dns-remote, then the
// fallbacks. Every attempt is logged once, under the upstream's tag.
func TestGuardFallsBackInOrder(t *testing.T) {
	shortGuard(t)
	l, b := debugLogger()
	primary := &fakeTransport{tag: GuardedTag}                                                                           // hangs
	google := &fakeTransport{tag: FallbackTags[0], steps: []func(*mDNS.Msg) (*mDNS.Msg, error){fail(errors.New("EOF"))}} // fails
	quad9 := &fakeTransport{tag: FallbackTags[1], steps: []func(*mDNS.Msg) (*mDNS.Msg, error){answer(mDNS.RcodeSuccess)}}
	response, err := guardWithFallbacks(t, l, primary, google, quad9).Exchange(context.Background(), query())
	if err != nil || response == nil || primary.calls != 1 || google.calls != 1 || quad9.calls != 1 {
		t.Fatalf("response %v, err %v, calls %d/%d/%d", response, err, primary.calls, google.calls, quad9.calls)
	}
	lines := b.String()
	for _, want := range []string{
		"server=dns-remote attempt=1 error=",
		"server=dns-remote-8.8.8.8 attempt=2 error=EOF",
		"server=dns-remote-9.9.9.9 attempt=3 rcode=NOERROR",
	} {
		if countLines(lines, want) != 1 {
			t.Fatalf("want %q once in:\n%s", want, lines)
		}
	}
	if n := countLines(lines, "msg=dns"); n != 3 {
		t.Fatalf("%d lines, want 3:\n%s", n, lines)
	}
}

// When every upstream fails the query fails: no other resolver is tried.
func TestGuardFailsWhenAllUpstreamsFail(t *testing.T) {
	shortGuard(t)
	primary, google, quad9 := &fakeTransport{tag: GuardedTag}, &fakeTransport{tag: FallbackTags[0]}, &fakeTransport{tag: FallbackTags[1]}
	if _, err := guardWithFallbacks(t, nil, primary, google, quad9).Exchange(context.Background(), query()); err == nil {
		t.Fatal("no error")
	}
	if primary.calls != 1 || google.calls != 1 || quad9.calls != 1 {
		t.Fatalf("calls %d/%d/%d", primary.calls, google.calls, quad9.calls)
	}
}

// After a fallback answers, queries start from it for preferFor, so a blocked
// dns-remote is not hit first every time; then dns-remote is tried first
// again. A failing preferred upstream falls through to the next and wraps
// around to dns-remote.
func TestGuardPrefersTheLastAnsweringUpstream(t *testing.T) {
	shortGuard(t)
	ok, eof := answer(mDNS.RcodeSuccess), fail(errors.New("EOF"))
	primary := &fakeTransport{tag: GuardedTag, steps: []func(*mDNS.Msg) (*mDNS.Msg, error){eof, ok, ok}}
	google := &fakeTransport{tag: FallbackTags[0], steps: []func(*mDNS.Msg) (*mDNS.Msg, error){ok, ok, eof}}
	quad9 := &fakeTransport{tag: FallbackTags[1], steps: []func(*mDNS.Msg) (*mDNS.Msg, error){eof}}
	guard := guardWithFallbacks(t, nil, primary, google, quad9)
	exchange := func() {
		t.Helper()
		if _, err := guard.Exchange(context.Background(), query()); err != nil {
			t.Fatal(err)
		}
	}
	calls := func() [3]int { return [3]int{primary.calls, google.calls, quad9.calls} }

	exchange() // dns-remote fails, 8.8.8.8 answers and becomes preferred
	if calls() != [3]int{1, 1, 0} {
		t.Fatalf("first query: %v", calls())
	}
	exchange() // starts at 8.8.8.8
	if calls() != [3]int{1, 2, 0} {
		t.Fatalf("preferred: %v", calls())
	}
	guard.preferredSince = guard.preferredSince.Add(-preferFor + time.Minute)
	exchange() // still within preferFor (fails, 9.9.9.9 fails, wraps to dns-remote)
	if calls() != [3]int{2, 3, 1} {
		t.Fatalf("wrap around: %v", calls())
	}
	if guard.preferred != 0 {
		t.Fatalf("preferred after dns-remote answered: %d", guard.preferred)
	}

	// Expiry: prefer 8.8.8.8, then let preferFor pass.
	guard.answered(1, time.Now().Add(-preferFor))
	primary.steps = append(primary.steps, ok)
	exchange()
	if calls() != [3]int{3, 3, 1} {
		t.Fatalf("after preferFor dns-remote is first again: %v", calls())
	}
}

// A fallback missing from the manager is warned about once, and looked up
// again on later queries instead of being lost for good.
func TestGuardWarnsAboutAMissingFallbackOnce(t *testing.T) {
	shortGuard(t)
	var b strings.Builder
	l := corelog.New(&b)
	ok := answer(mDNS.RcodeSuccess)
	primary := &fakeTransport{tag: GuardedTag, steps: []func(*mDNS.Msg) (*mDNS.Msg, error){ok, ok, ok}}
	google := &fakeTransport{tag: FallbackTags[0]}
	guard := guardWithFallbacks(t, l, primary, google) // no 9.9.9.9
	for range 2 {
		if _, err := guard.Exchange(context.Background(), query()); err != nil {
			t.Fatal(err)
		}
	}
	if n := strings.Count(b.String(), `level=warn msg="dns-remote fallback missing" servers=dns-remote-9.9.9.9`); n != 1 {
		t.Fatalf("%d warnings:\n%s", n, b.String())
	}
	if n := len(guard.upstreams); n != 2 {
		t.Fatalf("%d upstreams", n)
	}
	quad9 := &fakeTransport{tag: FallbackTags[1]}
	guard.manager.(*fakeManager).transports[quad9.tag] = quad9
	if _, err := guard.Exchange(context.Background(), query()); err != nil {
		t.Fatal(err)
	}
	if n := len(guard.upstreams); n != 3 {
		t.Fatalf("not resolved again: %d upstreams", n)
	}
}
