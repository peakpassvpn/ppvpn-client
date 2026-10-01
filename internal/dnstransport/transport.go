// Package dnstransport wraps the core's DNS transports:
//
//   - At debug level every attempt sent to an upstream server writes one line
//     to the first-party diagnostic log: the query name and type, the server
//     tag, the attempt number, the rcode (or the error) and how long it took.
//     Hijacked queries answered from the DNS cache never reach a transport and
//     are not logged.
//   - The remote server (GuardedTag) is guarded against half-open pooled
//     connections and falls back to the FallbackTags servers; see Exchange.
//
// The transports are registered under their usual type names, so options and
// golden files are unchanged; only the constructor wraps what sing-box builds.
package dnstransport

import (
	"context"
	"sync"
	"sync/atomic"
	"time"

	mDNS "github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/dns"
	"github.com/sagernet/sing-box/dns/transport"
	"github.com/sagernet/sing-box/dns/transport/local"
	"github.com/sagernet/sing-box/log"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/service"
)

// GuardedTag is the tag of the DNS server that is guarded: config's
// DNSRemoteTag (config cannot be imported here; a config test pins it).
const GuardedTag = "dns-remote"

// FallbackTags are the servers the guard falls back to, in order, after
// GuardedTag: config's DNSRemoteFallbackTags (pinned by a config test). They
// are ordinary servers to sing-box, referenced by no DNS rule.
var FallbackTags = []string{"dns-remote-8.8.8.8", "dns-remote-9.9.9.9"}

// Guard limits. The overall budget matches sing-box's DNS timeout.
var (
	attemptTimeout = 3 * time.Second
	overallBudget  = 10 * time.Second
	maxAttempts    = 3
	idleReset      = 30 * time.Second
	// preferFor is how long the guard keeps starting from the upstream that
	// last answered after a fallback, before trying GuardedTag first again.
	preferFor = 10 * time.Minute
)

type loggerKey struct{}

// WithLogger makes the DNS transports created under ctx log to l.
func WithLogger(ctx context.Context, l *corelog.Logger) context.Context {
	return context.WithValue(ctx, loggerKey{}, l)
}

func loggerFrom(ctx context.Context) *corelog.Logger {
	l, _ := ctx.Value(loggerKey{}).(*corelog.Logger)
	return l
}

// Register replaces the constructors of the transport types the core uses
// (and the plain remote ones its tests substitute) with wrapping ones.
func Register(registry *dns.TransportRegistry) {
	dns.RegisterTransport[option.LocalDNSServerOptions](registry, C.DNSTypeLocal, wrap(local.NewTransport))
	dns.RegisterTransport[option.RemoteTLSDNSServerOptions](registry, C.DNSTypeTLS, wrap(transport.NewTLS))
	dns.RegisterTransport[option.RemoteHTTPSDNSServerOptions](registry, C.DNSTypeHTTPS, wrap(transport.NewHTTPS))
	dns.RegisterTransport[option.RemoteDNSServerOptions](registry, C.DNSTypeTCP, wrap(transport.NewTCP))
	dns.RegisterTransport[option.RemoteDNSServerOptions](registry, C.DNSTypeUDP, wrap(transport.NewUDP))
}

func wrap[T any](constructor dns.TransportConstructorFunc[T]) dns.TransportConstructorFunc[T] {
	return func(ctx context.Context, logger log.ContextLogger, tag string, options T) (adapter.DNSTransport, error) {
		inner, err := constructor(ctx, logger, tag, options)
		if err != nil {
			return nil, err
		}
		l, guarded := loggerFrom(ctx), tag == GuardedTag
		if l == nil && !guarded {
			return inner, nil
		}
		w := &wrapped{DNSTransport: inner, log: l, guarded: guarded}
		if guarded {
			w.manager = service.FromContext[adapter.DNSTransportManager](ctx)
		}
		return w, nil
	}
}

// wrapped forwards everything to the inner transport. sing-box only inspects
// transports through Type() (fake-ip), which is forwarded.
type wrapped struct {
	adapter.DNSTransport
	log     *corelog.Logger
	guarded bool
	// inflight and lastSuccess (unix nanoseconds) drive the idle reset.
	inflight    atomic.Int32
	lastSuccess atomic.Int64
	// manager resolves FallbackTags on first use (they are created after
	// this transport).
	manager      adapter.DNSTransportManager
	upstreamOnce sync.Once
	upstreams    []adapter.DNSTransport
	// preferred indexes upstreams: where the next query starts, set when an
	// upstream other than the preferred one answers, at preferredSince.
	mu             sync.Mutex
	preferred      int
	preferredSince time.Time
}

// upstreamList returns the guarded transport followed by the fallbacks that
// exist, unwrapped so an attempt is logged once.
func (w *wrapped) upstreamList() []adapter.DNSTransport {
	w.upstreamOnce.Do(func() {
		w.upstreams = []adapter.DNSTransport{w.DNSTransport}
		if w.manager == nil {
			return
		}
		for _, tag := range FallbackTags {
			if upstream, ok := w.manager.Transport(tag); ok {
				if inner, ok := upstream.(*wrapped); ok {
					upstream = inner.DNSTransport
				}
				w.upstreams = append(w.upstreams, upstream)
			}
		}
	})
	return w.upstreams
}

// start returns the index of the upstream to try first.
func (w *wrapped) start(now time.Time) int {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.preferred != 0 && now.Sub(w.preferredSince) >= preferFor {
		w.preferred = 0
	}
	return w.preferred
}

// answered records that upstream index answered: a different upstream
// becomes the preferred one for preferFor.
func (w *wrapped) answered(index int, now time.Time) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if index != w.preferred {
		w.preferred, w.preferredSince = index, now
	}
}

// Exchange runs one attempt, or, for the guarded server, the guard:
//
// A pooled DoT connection whose path was silently dropped (a middlebox that
// forgets idle TCP flows without FIN/RST) swallows the query, and sing-box
// waits for the whole DNS timeout on it before its single retry, which then
// fails at once on the expired context. So each attempt gets attemptTimeout;
// a failed attempt's connection is discarded by the transport and the next
// attempt dials or takes another, within overallBudget and maxAttempts. Any
// transport error (timeout, EOF, reset) is retried; a DNS answer, including
// NXDOMAIN, is not an error. Before the first attempt after idleReset without
// a success and with nothing in flight, the pool is reset so a probably
// half-open idle connection is not reused.
//
// Each attempt goes to the next upstream: GuardedTag, then FallbackTags in
// order, wrapping around, so with three upstreams and three attempts each is
// tried once. An upstream that answers after a fallback is where queries
// start for preferFor, so a blocked upstream is not hit first on every query;
// then GuardedTag is tried first again. All upstreams failing fails the query.
func (w *wrapped) Exchange(ctx context.Context, message *mDNS.Msg) (*mDNS.Msg, error) {
	if !w.guarded {
		return w.attempt(ctx, w.DNSTransport, message, 0)
	}
	upstreams := w.upstreamList()
	if last := w.lastSuccess.Load(); last != 0 && time.Since(time.Unix(0, last)) > idleReset && w.inflight.Load() == 0 {
		for _, upstream := range upstreams {
			upstream.Reset()
		}
	}
	first := w.start(time.Now())
	w.inflight.Add(1)
	defer w.inflight.Add(-1)
	ctx, cancel := context.WithTimeout(ctx, overallBudget)
	defer cancel()
	var err error
	for attempt := 1; attempt <= maxAttempts; attempt++ {
		timeout := attemptTimeout
		if attempt == maxAttempts {
			timeout = overallBudget
		}
		attemptCtx, attemptCancel := context.WithTimeout(ctx, timeout)
		index := (first + attempt - 1) % len(upstreams)
		var response *mDNS.Msg
		response, err = w.attempt(attemptCtx, upstreams[index], message, attempt)
		attemptCancel()
		if err == nil {
			now := time.Now()
			w.lastSuccess.Store(now.UnixNano())
			w.answered(index, now)
			return response, nil
		}
		if ctx.Err() != nil {
			break
		}
	}
	return nil, err
}

// Start, Close and Reset reach the guarded transport only: sing-box manages
// the fallbacks as servers of their own.

// attempt runs one exchange on upstream and logs it at debug level, with the
// upstream's tag as server. attempt is 0 for an unguarded transport.
func (w *wrapped) attempt(ctx context.Context, upstream adapter.DNSTransport, message *mDNS.Msg, attempt int) (*mDNS.Msg, error) {
	if !w.log.DebugEnabled() {
		return upstream.Exchange(ctx, message)
	}
	started := time.Now()
	response, err := upstream.Exchange(ctx, message)
	name, qtype := "", ""
	if len(message.Question) > 0 {
		name, qtype = message.Question[0].Name, mDNS.TypeToString[message.Question[0].Qtype]
	}
	fields := []any{"name", name, "type", qtype, "server", upstream.Tag()}
	if attempt > 0 {
		fields = append(fields, "attempt", attempt)
	}
	switch {
	case err != nil:
		fields = append(fields, "error", err)
	case response != nil:
		fields = append(fields, "rcode", mDNS.RcodeToString[response.Rcode], "answers", len(response.Answer))
	}
	w.log.Debug("dns", append(fields, "ms", time.Since(started).Milliseconds())...)
	return response, err
}
