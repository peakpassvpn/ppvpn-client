// Package failover implements the ordered ingress group behind one logical
// node. Upstream sing-box offers "selector" (manual) and "urltest" (lowest
// latency with a tolerance); neither expresses "always prefer the primary,
// use a backup only while the primary is unhealthy, and return to the primary
// once it recovers". This group does exactly that:
//
//   - Dials go to the first healthy member in profile order (primary first).
//     A dial error that is not caused by the caller's context marks that
//     member unhealthy and the next member is tried immediately, so a dead
//     primary costs at most one failed dial before traffic moves to a backup.
//   - Health is an HTTP 204 check through each member (a plain HTTP exchange
//     performed on one goroutine, so it never closes a proxy connection while
//     another goroutine is still writing to it). Checks
//     only run while the group is in use (like urltest's idle timeout), so an
//     idle node never generates background traffic. While the primary is
//     healthy only the primary is checked; while it is unhealthy the primary
//     and every backup are checked on a shorter interval, and the group returns
//     to the primary as soon as a check succeeds.
//   - Existing connections are never interrupted by a switch.
//   - The group remembers which member carried its latest connection, the
//     member before the latest switch and when it switched (Active), and
//     reports every switch to a SwitchObserver found in its context.
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
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/log"
	E "github.com/sagernet/sing/common/exceptions"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/service"
)

// Type is the sing-box outbound type registered by Register.
const Type = "ppvpn-failover"

const (
	DefaultURL             = "http://www.gstatic.com/generate_204"
	DefaultInterval        = 3 * time.Minute
	DefaultRecoverInterval = 20 * time.Second
	DefaultIdleTimeout     = 30 * time.Minute
	checkTimeout           = 10 * time.Second
)

// Options is the rendered configuration of one logical node. Outbounds are
// ingress outbound tags in failover order; the first one is the primary.
type Options struct {
	Outbounds []string `json:"outbounds"`
	URL       string   `json:"url,omitempty"`
}

// Active describes the member that carried the group's latest connection.
// Before any connection it is the primary, with no previous member.
type Active struct {
	Current    string
	Previous   string
	SwitchedAt time.Time
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

type member struct {
	outbound adapter.Outbound
	healthy  atomic.Bool
}

type Group struct {
	outbound.Adapter
	ctx             context.Context
	cancel          context.CancelFunc
	logger          log.ContextLogger
	manager         adapter.OutboundManager
	tags            []string
	link            string
	members         []*member
	check           CheckFunc
	interval        time.Duration
	recoverInterval time.Duration
	idleTimeout     time.Duration
	observer        SwitchObserver

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
	link := options.URL
	if link == "" {
		link = DefaultURL
	}
	target, err := url.Parse(link)
	if err != nil || target.Scheme != "http" || target.Hostname() == "" {
		return nil, E.New("health check url must be an absolute http URL")
	}
	ctx, cancel := context.WithCancel(ctx)
	g := &Group{
		Adapter:         outbound.NewAdapter(Type, tag, []string{N.NetworkTCP, N.NetworkUDP}, slices.Clone(options.Outbounds)),
		ctx:             ctx,
		cancel:          cancel,
		logger:          logger,
		manager:         service.FromContext[adapter.OutboundManager](ctx),
		tags:            slices.Clone(options.Outbounds),
		link:            link,
		interval:        DefaultInterval,
		recoverInterval: DefaultRecoverInterval,
		idleTimeout:     DefaultIdleTimeout,
		wake:            make(chan struct{}, 1),
	}
	g.check = func(ctx context.Context, m adapter.Outbound) error { return httpCheck(ctx, target, m) }
	g.observer, _ = ctx.Value(observerKey{}).(SwitchObserver)
	g.active.Current = g.tags[0]
	return g, nil
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

// Now reports the member TCP traffic currently prefers.
func (g *Group) Now() string {
	if m := g.preferred(N.NetworkTCP); m != nil {
		return m.outbound.Tag()
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
	if g.observer != nil {
		g.observer(g.Tag(), active)
	}
}

func (g *Group) preferred(network string) *member {
	var fallback *member
	for _, m := range g.members {
		if !slices.Contains(m.outbound.Network(), network) {
			continue
		}
		if m.healthy.Load() {
			return m
		}
		if fallback == nil {
			fallback = m
		}
	}
	return fallback
}

// candidates lists healthy members in order followed by unhealthy ones as a
// last resort, so a group whose health data is stale never refuses traffic.
func (g *Group) candidates(network string) []*member {
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
	return append(healthy, unhealthy...)
}

func (g *Group) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	g.touch()
	var lastErr error
	for _, m := range g.candidates(N.NetworkName(network)) {
		conn, err := m.outbound.DialContext(ctx, network, destination)
		if err == nil {
			g.used(m)
			return conn, nil
		}
		lastErr = err
		if ctx.Err() != nil {
			return nil, err
		}
		g.markUnhealthy(m, err)
	}
	if lastErr == nil {
		lastErr = E.New("no ingress supports ", network)
	}
	return nil, lastErr
}

func (g *Group) ListenPacket(ctx context.Context, destination M.Socksaddr) (net.PacketConn, error) {
	g.touch()
	var lastErr error
	for _, m := range g.candidates(N.NetworkUDP) {
		conn, err := m.outbound.ListenPacket(ctx, destination)
		if err == nil {
			g.used(m)
			return conn, nil
		}
		lastErr = err
		if ctx.Err() != nil {
			return nil, err
		}
		g.markUnhealthy(m, err)
	}
	if lastErr == nil {
		lastErr = E.New("no ingress supports udp")
	}
	return nil, lastErr
}

func (g *Group) markUnhealthy(m *member, err error) {
	if errors.Is(err, context.Canceled) {
		return
	}
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

func (g *Group) degraded() bool { return !g.members[0].healthy.Load() }

func (g *Group) loop() {
	defer g.loops.Done()
	for {
		g.checkOnce()
		wait := g.interval
		if g.degraded() {
			wait = g.recoverInterval
		}
		timer := time.NewTimer(wait)
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

// checkOnce probes the primary; backups are probed only while the primary is
// unhealthy, keeping steady-state health traffic to one request per interval.
func (g *Group) checkOnce() {
	g.probe(g.members[0])
	if !g.degraded() || len(g.members) == 1 {
		return
	}
	var wg sync.WaitGroup
	for _, m := range g.members[1:] {
		wg.Add(1)
		go func(m *member) {
			defer wg.Done()
			g.probe(m)
		}(m)
	}
	wg.Wait()
}

func (g *Group) probe(m *member) {
	ctx, cancel := context.WithTimeout(g.ctx, min(checkTimeout, C.TCPTimeout))
	defer cancel()
	err := g.check(ctx, m.outbound)
	if g.ctx.Err() != nil {
		return
	}
	was := m.healthy.Swap(err == nil)
	if was != (err == nil) {
		if err == nil {
			g.logger.Info("ingress ", m.outbound.Tag(), " recovered")
		} else {
			g.logger.Info("ingress ", m.outbound.Tag(), " unhealthy: ", err)
		}
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
