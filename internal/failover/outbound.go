// Package failover implements the ordered ingress group behind one logical
// node. Upstream sing-box offers "selector" (manual) and "urltest" (lowest
// latency with a tolerance); neither expresses "prefer the primary, use a
// backup only while the primary is unhealthy, return to the primary once it
// has proven stable, and let the user pin one ingress". This group does:
//
//   - Dials go to the first healthy member in profile order (primary first).
//     Each member but the last gets DialTimeout; a dial error that is not
//     caused by the caller's context marks that member unhealthy and the next
//     member is tried within the same dial, so the application never sees it.
//     Unhealthy members stay as a last resort after the healthy ones.
//   - While the group is in use (touched within its idle timeout) every
//     member is health-checked each Interval: an HTTP 204 request through the
//     member to CheckURLs in turn (any 2xx/3xx passes; CheckTimeout each), so
//     an ingress that accepts connections but forwards nothing is found too.
//     UnhealthyAfter consecutive failed checks mark a member unhealthy, and
//     RecoverAfter consecutive passed checks mark it healthy again.
//   - Hysteresis: within MinDwell of an automatic switch the group keeps the
//     member it switched to while that member is healthy, so it does not flap
//     back to a recovering primary. Pinning or unpinning is an explicit
//     choice and clears it.
//   - A pinned member (Pin) carries every connection alone: no fallback, even
//     while it is unhealthy. Checks continue so its health is reported.
//   - Existing connections are never interrupted by a switch.
//   - The group remembers which member carried its latest connection, the
//     member before the latest switch and when it switched (Active), reports
//     every switch to a SwitchObserver found in its context, and reports each
//     member's health (Members).
package failover

import (
	"bufio"
	"context"
	"errors"
	"net"
	"net/http"
	"net/url"
	"slices"
	"sync"
	"sync/atomic"
	"time"

	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing-box/adapter/outbound"
	"github.com/sagernet/sing-box/log"
	E "github.com/sagernet/sing/common/exceptions"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/service"
)

// Type is the sing-box outbound type registered by Register.
const Type = "ppvpn-failover"

// CheckURLs are tried in order; a member passes when any answers 2xx/3xx.
// gstatic is the primary check, Cloudflare the fallback; both were verified
// reachable from mainland China and overseas exits.
var CheckURLs = []string{"http://www.gstatic.com/generate_204", "http://cp.cloudflare.com/generate_204"}

const (
	DialTimeout        = 2 * time.Second
	Interval           = 15 * time.Second
	CheckTimeout       = 5 * time.Second
	UnhealthyAfter     = 2
	RecoverAfter       = 3
	MinDwell           = 60 * time.Second
	DefaultIdleTimeout = 30 * time.Minute
)

// Options is the rendered configuration of one logical node. Outbounds are
// ingress outbound tags in failover order; the first one is the primary.
// URLs overrides CheckURLs (tests).
type Options struct {
	Outbounds []string `json:"outbounds"`
	URLs      []string `json:"urls,omitempty"`
}

// Active describes the member that carried the group's latest connection.
// Before any connection it is the primary, with no previous member.
type Active struct {
	Current    string
	Previous   string
	SwitchedAt time.Time
}

// MemberStatus is one member's health. LastCheck is zero before its first
// check; members of an idle group are not checked.
type MemberStatus struct {
	Tag                 string
	Healthy             bool
	LastCheck           time.Time
	ConsecutiveFailures int
}

// SwitchObserver is told about every switch; it must not block.
type SwitchObserver func(group string, active Active)

type observerKey struct{}

// WithSwitchObserver returns a context whose failover groups report switches
// to observer.
func WithSwitchObserver(ctx context.Context, observer SwitchObserver) context.Context {
	return context.WithValue(ctx, observerKey{}, observer)
}

// Register adds the failover outbound type to a sing-box outbound registry.
func Register(registry *outbound.Registry) {
	outbound.Register[Options](registry, Type, New)
}

// CheckFunc reports whether a member can carry traffic.
type CheckFunc func(ctx context.Context, member adapter.Outbound) error

// ErrUnknownMember rejects a pin to a tag outside the group.
var ErrUnknownMember = errors.New("not a member of the group")

type member struct {
	outbound adapter.Outbound
	healthy  atomic.Bool

	mu        sync.Mutex
	failures  int
	successes int
	lastCheck time.Time
}

type Group struct {
	outbound.Adapter
	ctx       context.Context
	cancel    context.CancelFunc
	logger    log.ContextLogger
	manager   adapter.OutboundManager
	tags      []string
	members   []*member
	check     CheckFunc
	observer  SwitchObserver
	pinned    atomic.Pointer[member]
	pinnedTag atomic.Value // string: pin requested before Start
	// dwellSince is when automatic selection last switched members (unix
	// nanoseconds; 0 for none); see candidates.
	dwellSince  atomic.Int64
	interval    time.Duration
	dialTimeout time.Duration
	minDwell    time.Duration
	idleTimeout time.Duration

	activeMu sync.Mutex
	active   Active

	started    atomic.Bool
	lastActive atomic.Int64
	wake       chan struct{}
	loopMu     sync.Mutex
	looping    bool
	closed     bool
	loops      sync.WaitGroup
}

var (
	_ adapter.OutboundGroup = (*Group)(nil)
	_ adapter.Lifecycle     = (*Group)(nil)
)

func New(ctx context.Context, _ adapter.Router, logger log.ContextLogger, tag string, options Options) (adapter.Outbound, error) {
	if len(options.Outbounds) == 0 {
		return nil, E.New("missing ingress outbounds")
	}
	for i, t := range options.Outbounds {
		if t == "" || t == tag || slices.Contains(options.Outbounds[:i], t) {
			return nil, E.New("invalid ingress outbound ", i)
		}
	}
	links := options.URLs
	if len(links) == 0 {
		links = CheckURLs
	}
	targets := make([]*url.URL, len(links))
	for i, link := range links {
		target, err := url.Parse(link)
		if err != nil || target.Scheme != "http" || target.Hostname() == "" {
			return nil, E.New("health check url must be an absolute http URL")
		}
		targets[i] = target
	}
	ctx, cancel := context.WithCancel(ctx)
	g := &Group{
		Adapter:     outbound.NewAdapter(Type, tag, []string{N.NetworkTCP, N.NetworkUDP}, slices.Clone(options.Outbounds)),
		ctx:         ctx,
		cancel:      cancel,
		logger:      logger,
		manager:     service.FromContext[adapter.OutboundManager](ctx),
		tags:        slices.Clone(options.Outbounds),
		interval:    Interval,
		dialTimeout: DialTimeout,
		minDwell:    MinDwell,
		idleTimeout: DefaultIdleTimeout,
		wake:        make(chan struct{}, 1),
	}
	g.check = func(ctx context.Context, m adapter.Outbound) error { return checkAny(ctx, targets, m) }
	g.observer, _ = ctx.Value(observerKey{}).(SwitchObserver)
	g.active.Current = g.tags[0]
	return g, nil
}

// checkAny passes when any target answers through member, each within
// CheckTimeout.
func checkAny(ctx context.Context, targets []*url.URL, member adapter.Outbound) error {
	var err error
	for _, target := range targets {
		checkCtx, cancel := context.WithTimeout(ctx, CheckTimeout)
		err = httpCheck(checkCtx, target, member)
		cancel()
		if err == nil || ctx.Err() != nil {
			return err
		}
	}
	return err
}

func (g *Group) Start(stage adapter.StartStage) error {
	switch stage {
	case adapter.StartStateStart:
		if g.manager == nil {
			return E.New("missing outbound manager")
		}
		g.members = make([]*member, 0, len(g.tags))
		for i, tag := range g.tags {
			detour, ok := g.manager.Outbound(tag)
			if !ok {
				return E.New("ingress outbound ", i, " not found: ", tag)
			}
			m := &member{outbound: detour}
			m.healthy.Store(true)
			g.members = append(g.members, m)
		}
		if tag, _ := g.pinnedTag.Load().(string); tag != "" {
			if err := g.Pin(tag); err != nil {
				return err
			}
		}
	case adapter.StartStateStarted:
		g.started.Store(true)
	}
	return nil
}

func (g *Group) Close() error {
	g.cancel()
	g.loopMu.Lock()
	g.closed = true
	g.loopMu.Unlock()
	g.loops.Wait()
	return nil
}

// Pin makes the member tagged tag carry every connection, with no fallback;
// an empty tag returns the group to automatic selection. Before Start the pin
// is remembered and applied when the members exist.
func (g *Group) Pin(tag string) error {
	if len(g.members) == 0 {
		if tag != "" && !slices.Contains(g.tags, tag) {
			return ErrUnknownMember
		}
		g.pinnedTag.Store(tag)
		return nil
	}
	if tag == "" {
		g.pinned.Store(nil)
		g.pinnedTag.Store("")
		g.dwellSince.Store(0)
		return nil
	}
	for _, m := range g.members {
		if m.outbound.Tag() == tag {
			g.pinned.Store(m)
			g.pinnedTag.Store(tag)
			g.dwellSince.Store(0)
			return nil
		}
	}
	return ErrUnknownMember
}

// Pinned reports the pinned member's tag, or "" in automatic selection.
func (g *Group) Pinned() string {
	if m := g.pinned.Load(); m != nil {
		return m.outbound.Tag()
	}
	tag, _ := g.pinnedTag.Load().(string)
	return tag
}

// Members reports every member's health in profile order.
func (g *Group) Members() []MemberStatus {
	out := make([]MemberStatus, 0, len(g.members))
	for _, m := range g.members {
		m.mu.Lock()
		out = append(out, MemberStatus{Tag: m.outbound.Tag(), Healthy: m.healthy.Load(), LastCheck: m.lastCheck, ConsecutiveFailures: m.failures})
		m.mu.Unlock()
	}
	return out
}

// Now reports the member TCP traffic currently prefers.
func (g *Group) Now() string {
	if candidates := g.candidates(N.NetworkTCP); len(candidates) > 0 {
		return candidates[0].outbound.Tag()
	}
	return ""
}

func (g *Group) All() []string { return slices.Clone(g.tags) }

// Active reports the member in use and the latest switch.
func (g *Group) Active() Active {
	g.activeMu.Lock()
	defer g.activeMu.Unlock()
	return g.active
}

// used records that m carried a new connection.
func (g *Group) used(m *member) {
	tag := m.outbound.Tag()
	g.activeMu.Lock()
	if g.active.Current == tag {
		g.activeMu.Unlock()
		return
	}
	g.active = Active{Current: tag, Previous: g.active.Current, SwitchedAt: time.Now()}
	active := g.active
	g.activeMu.Unlock()
	if g.pinned.Load() == nil {
		g.dwellSince.Store(active.SwitchedAt.UnixNano())
	}
	if g.observer != nil {
		g.observer(g.Tag(), active)
	}
}

// candidates lists the members to try in order: the pinned member alone, or
// the healthy members in profile order followed by the unhealthy ones as a
// last resort, so a group whose health data is stale never refuses traffic.
// Within minDwell of a switch the member switched to leads while healthy.
func (g *Group) candidates(network string) []*member {
	if m := g.pinned.Load(); m != nil {
		if slices.Contains(m.outbound.Network(), network) {
			return []*member{m}
		}
		return nil
	}
	healthy := make([]*member, 0, len(g.members))
	var unhealthy []*member
	for _, m := range g.members {
		if !slices.Contains(m.outbound.Network(), network) {
			continue
		}
		if m.healthy.Load() {
			healthy = append(healthy, m)
		} else {
			unhealthy = append(unhealthy, m)
		}
	}
	if since := g.dwellSince.Load(); since != 0 && time.Since(time.Unix(0, since)) < g.minDwell {
		active := g.Active()
		for i, m := range healthy {
			if i > 0 && m.outbound.Tag() == active.Current {
				healthy = append([]*member{m}, append(healthy[:i:i], healthy[i+1:]...)...)
				break
			}
		}
	}
	return append(healthy, unhealthy...)
}

// dial tries each candidate; every one but the last gets dialTimeout.
func dialCandidates[T any](g *Group, ctx context.Context, network string, dial func(context.Context, *member) (T, error)) (T, error) {
	g.touch()
	var zero T
	var lastErr error
	candidates := g.candidates(network)
	for i, m := range candidates {
		attemptCtx, cancel := ctx, context.CancelFunc(func() {})
		if i < len(candidates)-1 {
			attemptCtx, cancel = context.WithTimeout(ctx, g.dialTimeout)
		}
		conn, err := dial(attemptCtx, m)
		cancel()
		if err == nil {
			g.used(m)
			return conn, nil
		}
		lastErr = err
		if ctx.Err() != nil {
			return zero, err
		}
		g.markUnhealthy(m, err)
	}
	if lastErr == nil {
		lastErr = E.New("no ingress supports ", network)
	}
	return zero, lastErr
}

func (g *Group) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	return dialCandidates(g, ctx, N.NetworkName(network), func(ctx context.Context, m *member) (net.Conn, error) {
		return m.outbound.DialContext(ctx, network, destination)
	})
}

func (g *Group) ListenPacket(ctx context.Context, destination M.Socksaddr) (net.PacketConn, error) {
	return dialCandidates(g, ctx, N.NetworkUDP, func(ctx context.Context, m *member) (net.PacketConn, error) {
		return m.outbound.ListenPacket(ctx, destination)
	})
}

// markUnhealthy handles a failed dial: decisive, so the member is unhealthy
// at once and must pass RecoverAfter checks to come back.
func (g *Group) markUnhealthy(m *member, err error) {
	if errors.Is(err, context.Canceled) {
		return
	}
	m.mu.Lock()
	m.successes = 0
	m.failures = max(m.failures, UnhealthyAfter)
	m.mu.Unlock()
	if m.healthy.Swap(false) {
		g.logger.Debug("ingress ", m.outbound.Tag(), " marked unhealthy: ", err)
	}
	g.signal()
}

func (g *Group) signal() {
	select {
	case g.wake <- struct{}{}:
	default:
	}
}

// touch records activity and lazily starts the health loop.
func (g *Group) touch() {
	g.lastActive.Store(time.Now().UnixNano())
	if !g.started.Load() {
		return
	}
	g.loopMu.Lock()
	defer g.loopMu.Unlock()
	if g.looping || g.closed {
		return
	}
	g.looping = true
	g.loops.Add(1)
	go g.loop()
}

func (g *Group) idle() bool {
	return time.Since(time.Unix(0, g.lastActive.Load())) > g.idleTimeout
}

func (g *Group) loop() {
	defer g.loops.Done()
	for {
		g.checkOnce()
		timer := time.NewTimer(g.interval)
		select {
		case <-g.ctx.Done():
			timer.Stop()
			g.stopLoop()
			return
		case <-timer.C:
		case <-g.wake:
			timer.Stop()
		}
		g.loopMu.Lock()
		if g.idle() || g.ctx.Err() != nil {
			g.looping = false
			g.loopMu.Unlock()
			return
		}
		g.loopMu.Unlock()
	}
}

func (g *Group) stopLoop() {
	g.loopMu.Lock()
	g.looping = false
	g.loopMu.Unlock()
}

// checkOnce checks every member concurrently.
func (g *Group) checkOnce() {
	var wg sync.WaitGroup
	for _, m := range g.members {
		wg.Add(1)
		go func(m *member) {
			defer wg.Done()
			g.probe(m)
		}(m)
	}
	wg.Wait()
}

func (g *Group) probe(m *member) {
	err := g.check(g.ctx, m.outbound)
	if g.ctx.Err() != nil {
		return
	}
	m.mu.Lock()
	m.lastCheck = time.Now()
	if err == nil {
		m.failures, m.successes = 0, m.successes+1
	} else {
		m.failures, m.successes = m.failures+1, 0
	}
	failures, successes := m.failures, m.successes
	m.mu.Unlock()
	switch {
	case err == nil && successes >= RecoverAfter && !m.healthy.Load():
		m.healthy.Store(true)
		g.logger.Info("ingress ", m.outbound.Tag(), " recovered")
	case err != nil && failures >= UnhealthyAfter && m.healthy.Swap(false):
		g.logger.Info("ingress ", m.outbound.Tag(), " unhealthy: ", err)
	}
}

// httpCheck sends one HEAD request through member and expects a 2xx/3xx.
func httpCheck(ctx context.Context, target *url.URL, member adapter.Outbound) error {
	port := target.Port()
	if port == "" {
		port = "80"
	}
	conn, err := member.DialContext(ctx, N.NetworkTCP, M.ParseSocksaddrHostPortStr(target.Hostname(), port))
	if err != nil {
		return err
	}
	defer conn.Close()
	if deadline, ok := ctx.Deadline(); ok {
		_ = conn.SetDeadline(deadline)
	}
	stop := context.AfterFunc(ctx, func() { _ = conn.SetDeadline(time.Unix(1, 0)) })
	defer stop()
	request := "HEAD " + target.RequestURI() + " HTTP/1.1\r\nHost: " + target.Host + "\r\nUser-Agent: ppvpn-core\r\nConnection: close\r\n\r\n"
	if _, err = conn.Write([]byte(request)); err != nil {
		return err
	}
	response, err := http.ReadResponse(bufio.NewReader(conn), &http.Request{Method: http.MethodHead})
	if err != nil {
		return err
	}
	_ = response.Body.Close()
	if response.StatusCode < 200 || response.StatusCode >= 400 {
		return E.New("health check status ", response.StatusCode)
	}
	return nil
}
