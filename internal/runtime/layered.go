package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"reflect"
	"sync"
	"sync/atomic"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/internal/proxyinbound"
	"github.com/peakpassvpn/ppvpn-core/internal/reversemap"
	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing-tun"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/service"
)

// A layered engine keeps the listeners in a long-lived front box and does
// everything else (outbounds, route rules, DNS, sniffing, domaindest) in a
// kernel box with no inbounds. The inbounds route through a switchRouter
// that hands each new connection to the current kernel's router, so an
// apply-profile can start a new kernel and switch to it without closing a
// listener or the TUN: connections already routed stay in the old kernel,
// which drains (until they end, or drainLimit) and is then closed.
//
// Only a change to the listeners themselves (fullRestartReasons) needs the
// old stop-and-start path.

// drainLimit bounds how long a replaced kernel keeps its connections.
var drainLimit = 10 * time.Minute

// drainCheckInterval is how often draining kernels are checked.
var drainCheckInterval = time.Second

// beforeKernelStart lets tests fail a kernel's start.
var beforeKernelStart func() error

// drainGrace keeps a replaced kernel at least this long: a connection it
// was routing at the switch (still sniffing) registers with the tracker only
// once routed, so an empty count right after the switch is not final.
var drainGrace = 5 * time.Second

// kernelEvent reports a switch or a drained kernel to the core.
type kernelEvent struct {
	Switched bool // a new kernel took over (Gen); otherwise Gen drained
	Gen      uint64
	Previous uint64
	// Closed: connections closed (on a switch, by the new profile; on a
	// drain, still open at drainLimit). Kept: left to drain on a switch.
	Closed, Kept int
	// Reason of a drain: "idle" (no connections left) or "deadline".
	Reason string
}

// swapEngine is an engine that can replace its kernel in place.
type swapEngine interface {
	// fullRestartReasons lists why options cannot be swapped in, empty when
	// they can.
	fullRestartReasons(options option.Options) []string
	// swap starts a kernel for options (prepare runs before it starts) and
	// switches new connections to it. closeOld decides, with the new
	// kernel, which of the old kernel's connections to close.
	swap(ctx context.Context, options option.Options, prepare func(engine), closeOld func(next engine, item trackedView) bool) (kernelEvent, error)
	drainingKernels() int
	setKernelEvents(func(kernelEvent))
}

type kernel struct {
	*singEngine
	// reverseMapping: this kernel's DNS has dns.reverse_mapping on (the TUN
	// configuration), so its router fills domains of address destinations.
	reverseMapping bool
	gen            uint64
	cancel         context.CancelFunc
	retired        time.Time
	deadline       time.Time
}

type layeredEngine struct {
	ctx      context.Context
	front    *box.Box
	switcher *switchRouter
	tracker  *telemetry
	// inbounds are the front's listeners as built (system-proxy toggles go
	// through addInbound/removeInbound).
	inbounds []option.Inbound
	route    *option.RouteOptions
	// Drain timing, copied from the package variables when built (tests
	// change those; the drain loop must not read them concurrently).
	drainLimit, drainGrace, drainCheck time.Duration

	// active is the current kernel. It is read without mu: the core reads it
	// while holding its own lock (Status, probes, pins), and swap's prepare
	// takes that lock, so mu must never be needed to reach the kernel.
	active atomic.Pointer[kernel]
	gen    atomic.Uint64

	// mu guards the switch itself, draining, inbounds and started.
	mu       sync.Mutex
	draining []*kernel
	// drainingCount mirrors len(draining) for readers that must not take mu
	// (Status holds the core's lock, which swap's prepare takes under mu).
	drainingCount atomic.Int32
	started       bool
	stop          chan struct{}
	events        atomic.Pointer[func(kernelEvent)]
}

// newLayeredEngine builds (does not start) the front and the first kernel.
// ctx must not carry a service registry: each box needs its own, and
// failover.Context creates one per call only on a context without one.
func newLayeredEngine(ctx context.Context, options option.Options) (*layeredEngine, error) {
	frontCtx := failover.Context(ctx)
	tracker := newTelemetry()
	front, err := box.New(box.Options{Context: frontCtx, Options: frontOptions(options)})
	if err != nil {
		return nil, err
	}
	engine := &layeredEngine{ctx: frontCtx, front: front, switcher: &switchRouter{reverse: reversemap.FromContext(ctx)}, tracker: tracker,
		inbounds: options.Inbounds, route: options.Route, stop: make(chan struct{}),
		drainLimit: drainLimit, drainGrace: drainGrace, drainCheck: drainCheckInterval}
	first, err := engine.newKernel(ctx, options)
	if err != nil {
		_ = front.Close()
		return nil, err
	}
	engine.active.Store(first)
	return engine, nil
}

// frontOptions is what the listeners' host box needs: logging and the
// interface options the TUN inbound reads, no rules, no DNS, no inbounds
// (they are created with the switchRouter after start).
func frontOptions(options option.Options) option.Options {
	front := option.Options{Log: options.Log}
	if options.Route != nil {
		front.Route = &option.RouteOptions{
			AutoDetectInterface: options.Route.AutoDetectInterface,
			OverrideAndroidVPN:  options.Route.OverrideAndroidVPN,
			DefaultInterface:    options.Route.DefaultInterface,
			DefaultMark:         options.Route.DefaultMark,
		}
	}
	return front
}

func kernelOptions(options option.Options) option.Options {
	options.Inbounds = nil
	return options
}

func (e *layeredEngine) newKernel(ctx context.Context, options option.Options) (*kernel, error) {
	gen := e.gen.Add(1)
	kernelCtx, cancel := context.WithCancel(failover.Context(ctx))
	instance, err := box.New(box.Options{Context: kernelCtx, Options: kernelOptions(options)})
	if err != nil {
		cancel()
		return nil, err
	}
	instance.Router().AppendTracker(kernelTracker{t: e.tracker, gen: gen, outbounds: instance.Outbound()})
	reverseMapping := options.DNS != nil && options.DNS.ReverseMapping
	return &kernel{singEngine: &singEngine{Box: instance, ctx: kernelCtx, tracker: e.tracker}, reverseMapping: reverseMapping, gen: gen, cancel: cancel}, nil
}

func (k *kernel) close() {
	k.cancel()
	_ = k.Box.Close()
}

func (e *layeredEngine) Start() error {
	e.mu.Lock()
	defer e.mu.Unlock()
	first := e.active.Load()
	if err := first.Start(); err != nil {
		return err
	}
	e.switcher.set(first)
	if err := e.front.Start(); err != nil {
		return err
	}
	for _, inbound := range e.inbounds {
		if err := e.createInbound(inbound); err != nil {
			return err
		}
	}
	e.registerTUNInterface(first)
	e.started = true
	go e.drainLoop()
	return nil
}

func (e *layeredEngine) createInbound(inbound option.Inbound) error {
	return e.front.Inbound().Create(e.ctx, e.switcher, e.front.LogFactory().NewLogger("inbound/"+inbound.Tag), inbound.Tag, inbound.Type, inbound.Options)
}

// registerTUNInterface tells the kernel's interface monitor which interface
// is the front's TUN, as the TUN inbound tells the monitor of its own box
// (sing-box skips it, e.g. for the Windows local resolver).
func (e *layeredEngine) registerTUNInterface(k *kernel) {
	front := service.FromContext[adapter.NetworkManager](e.ctx)
	kernel := service.FromContext[adapter.NetworkManager](k.ctx)
	if front == nil || kernel == nil || front.InterfaceMonitor() == nil || kernel.InterfaceMonitor() == nil {
		return
	}
	if name := front.InterfaceMonitor().MyInterface(); name != "" {
		kernel.InterfaceMonitor().RegisterMyInterface(name)
	}
}

func (e *layeredEngine) Close() error {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.started {
		close(e.stop)
		e.started = false
	}
	// Listeners first, so nothing new reaches a kernel being closed.
	err := e.front.Close()
	for _, k := range e.draining {
		k.close()
	}
	e.draining = nil
	e.drainingCount.Store(0)
	if current := e.active.Load(); current != nil {
		current.close()
	}
	return err
}

// kernel is the current kernel; during a swap that is still the old one
// until the switch.
func (e *layeredEngine) kernel() *kernel { return e.active.Load() }

func (e *layeredEngine) setKernelEvents(handler func(kernelEvent)) { e.events.Store(&handler) }

func (e *layeredEngine) emit(event kernelEvent) {
	if handler := e.events.Load(); handler != nil {
		(*handler)(event)
	}
}

// fullRestartReasons is the whitelist of changes the front cannot take in
// place: the TUN inbound's options, the local proxy listener's address or
// port, any other inbound, and the interface options the TUN uses. The local
// proxy's users (usernames and the shared password) are replaced in place
// and the system proxy listener is reconciled in place.
func (e *layeredEngine) fullRestartReasons(options option.Options) []string {
	var reasons []string
	current, next := inboundsByTag(e.inbounds), inboundsByTag(options.Inbounds)
	for tag := range union(current, next) {
		if tag == config.SystemProxyInboundTag {
			continue
		}
		before, hadBefore := current[tag]
		after, hasAfter := next[tag]
		switch {
		case !hadBefore || !hasAfter:
			reasons = append(reasons, "inbound "+tag+" added or removed")
		case tag == config.TUNInboundTag && !sameInbound(before, after):
			reasons = append(reasons, "tun options changed")
		case before.Type == proxyinbound.Type && !sameInbound(withoutUsers(before), withoutUsers(after)):
			reasons = append(reasons, "local proxy listener changed")
		case before.Type != proxyinbound.Type && tag != config.TUNInboundTag && !sameInbound(before, after):
			reasons = append(reasons, "inbound "+tag+" changed")
		}
	}
	if !sameJSON(frontOptions(option.Options{Route: e.route}).Route, frontOptions(options).Route) {
		reasons = append(reasons, "interface options changed")
	}
	return reasons
}

func inboundsByTag(inbounds []option.Inbound) map[string]option.Inbound {
	out := make(map[string]option.Inbound, len(inbounds))
	for _, inbound := range inbounds {
		out[inbound.Tag] = inbound
	}
	return out
}

func union(a, b map[string]option.Inbound) map[string]struct{} {
	out := map[string]struct{}{}
	for tag := range a {
		out[tag] = struct{}{}
	}
	for tag := range b {
		out[tag] = struct{}{}
	}
	return out
}

func withoutUsers(inbound option.Inbound) option.Inbound {
	if options, ok := inbound.Options.(*proxyinbound.Options); ok {
		copied := *options
		copied.Users = nil
		inbound.Options = &copied
	}
	return inbound
}

// sameInbound compares type and options. option.Inbound's own JSON needs the
// options registry from a context and leaves Options out without one, so the
// options struct is compared directly.
func sameInbound(a, b option.Inbound) bool {
	return a.Type == b.Type && sameJSON(a.Options, b.Options)
}

func sameJSON(a, b any) bool {
	left, errLeft := json.Marshal(a)
	right, errRight := json.Marshal(b)
	return errLeft == nil && errRight == nil && reflect.DeepEqual(left, right)
}

// swap starts a kernel for options and switches to it. If the kernel fails
// to start, it is discarded and nothing else changes.
//
// The new kernel is built, prepared (pins: the core's lock) and started
// without mu: nobody else can see it yet, and mu must not be held while the
// core's lock is taken (Status reads the kernel under that lock). mu covers
// only the switch.
func (e *layeredEngine) swap(ctx context.Context, options option.Options, prepare func(engine), closeOld func(next engine, item trackedView) bool) (kernelEvent, error) {
	e.mu.Lock()
	started := e.started
	e.mu.Unlock()
	if !started {
		return kernelEvent{}, errors.New("engine is not running")
	}
	next, err := e.newKernel(ctx, options)
	if err != nil {
		return kernelEvent{}, err
	}
	if beforeKernelStart != nil {
		err = beforeKernelStart()
	}
	if err == nil {
		if prepare != nil {
			prepare(next.singEngine)
		}
		err = next.Start()
	}
	if err != nil {
		next.close()
		return kernelEvent{}, err
	}

	e.mu.Lock()
	defer e.mu.Unlock()
	if !e.started {
		next.close()
		return kernelEvent{}, errors.New("engine closed during the switch")
	}
	e.registerTUNInterface(next)
	if err = e.reconcileInbounds(options.Inbounds); err != nil {
		next.close()
		return kernelEvent{}, fmt.Errorf("update listeners: %w", err)
	}
	previous := e.active.Load()
	e.switcher.set(next)
	e.active.Store(next)
	e.inbounds = options.Inbounds
	event := kernelEvent{Switched: true, Gen: next.gen, Previous: previous.gen}
	for _, item := range e.tracker.generation(previous.gen) {
		if closeOld != nil && closeOld(next.singEngine, item) {
			e.tracker.closeConnection(item.item)
			event.Closed++
		} else {
			event.Kept++
		}
	}
	previous.retired = time.Now()
	previous.deadline = previous.retired.Add(e.drainLimit)
	e.draining = append(e.draining, previous)
	e.drainingCount.Store(int32(len(e.draining)))
	return event, nil
}

// reconcileInbounds applies the in-place listener changes: the local proxy's
// user list, and the system proxy listener (added, moved or removed). If the
// system proxy change fails, the user lists are put back, so the listeners
// still match the kernel that stays (whose rules pin the old users).
func (e *layeredEngine) reconcileInbounds(inbounds []option.Inbound) (err error) {
	next := inboundsByTag(inbounds)
	current := inboundsByTag(e.inbounds)
	var restore []func()
	defer func() {
		if err != nil {
			for _, undo := range restore {
				undo()
			}
		}
	}()
	for _, inbound := range inbounds {
		options, ok := inbound.Options.(*proxyinbound.Options)
		if !ok {
			continue
		}
		existing, ok := e.front.Inbound().Get(inbound.Tag)
		if !ok {
			continue
		}
		if setter, ok := existing.(interface {
			SetUsers([]proxyinbound.User) error
		}); ok {
			if err := setter.SetUsers(options.Users); err != nil {
				return err
			}
			if old, ok := current[inbound.Tag].Options.(*proxyinbound.Options); ok {
				restore = append(restore, func() { _ = setter.SetUsers(old.Users) })
			}
		}
	}
	before, had := current[config.SystemProxyInboundTag]
	after, has := next[config.SystemProxyInboundTag]
	switch {
	case has && (!had || !sameInbound(before, after)):
		if had {
			if err := e.front.Inbound().Remove(config.SystemProxyInboundTag); err != nil {
				return err
			}
		}
		return e.createInbound(after)
	case had && !has:
		return e.front.Inbound().Remove(config.SystemProxyInboundTag)
	}
	return nil
}

func (e *layeredEngine) drainingKernels() int { return int(e.drainingCount.Load()) }

func (e *layeredEngine) drainLoop() {
	ticker := time.NewTicker(e.drainCheck)
	defer ticker.Stop()
	for {
		select {
		case <-e.stop:
			return
		case <-ticker.C:
			e.drainOnce(time.Now())
		}
	}
}

// drainOnce closes every draining kernel with no connections left, or past
// its deadline (closing what is still open).
func (e *layeredEngine) drainOnce(now time.Time) {
	e.mu.Lock()
	var done []kernelEvent
	var closing []*kernel
	kept := make([]*kernel, 0, len(e.draining))
	for _, k := range e.draining {
		open := len(e.tracker.generation(k.gen))
		switch {
		case open == 0 && now.Sub(k.retired) >= e.drainGrace:
			done = append(done, kernelEvent{Gen: k.gen, Reason: "idle"})
		case !now.Before(k.deadline):
			done = append(done, kernelEvent{Gen: k.gen, Reason: "deadline", Closed: open})
		default:
			kept = append(kept, k)
			continue
		}
		closing = append(closing, k)
	}
	e.draining = kept
	e.drainingCount.Store(int32(len(kept)))
	e.mu.Unlock()
	// Outside the lock: closing a box can take a while, and closes the
	// connections still open in it.
	for _, k := range closing {
		k.close()
	}
	for _, event := range done {
		e.emit(event)
	}
}

// The engine capabilities go to the current kernel; listeners to the front.

func (e *layeredEngine) ingressGroup(nodeTag string) (*failover.Group, bool) {
	return e.kernel().ingressGroup(nodeTag)
}
func (e *layeredEngine) activeIngress(nodeTag string) (failover.Active, bool) {
	return e.kernel().activeIngress(nodeTag)
}
func (e *layeredEngine) addInbound(inbound option.Inbound) error {
	e.mu.Lock()
	defer e.mu.Unlock()
	if err := e.createInbound(inbound); err != nil {
		return err
	}
	e.inbounds = append(withoutTag(e.inbounds, inbound.Tag), inbound)
	return nil
}
func (e *layeredEngine) removeInbound(tag string) error {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.inbounds = withoutTag(e.inbounds, tag)
	if _, ok := e.front.Inbound().Get(tag); !ok {
		return nil
	}
	return e.front.Inbound().Remove(tag)
}

func withoutTag(inbounds []option.Inbound, tag string) []option.Inbound {
	out := make([]option.Inbound, 0, len(inbounds))
	for _, inbound := range inbounds {
		if inbound.Tag != tag {
			out = append(out, inbound)
		}
	}
	return out
}

func (e *layeredEngine) setConnectionLog(log *corelog.Logger) { e.tracker.log.Store(log) }

// watchDefaultInterface watches the front's monitor: it lives as long as the
// engine, while kernels are replaced.
func (e *layeredEngine) watchDefaultInterface(log *corelog.Logger) {
	(&singEngine{Box: e.front, ctx: e.ctx}).watchDefaultInterface(log)
}
func (e *layeredEngine) telemetrySnapshot() (Traffic, []Connection) { return e.tracker.snapshot() }
func (e *layeredEngine) dialFlow(ctx context.Context, network, outboundTag, host string, port uint16) (net.Conn, error) {
	return e.kernel().dialFlow(ctx, network, outboundTag, host, port)
}
func (e *layeredEngine) dialDirect(ctx context.Context, network, address string) (net.Conn, error) {
	return e.kernel().dialDirect(ctx, network, address)
}
func (e *layeredEngine) selectOutbound(outboundTag string) bool {
	return e.kernel().selectOutbound(outboundTag)
}

// switchRouter is the router the front's inbounds were created with. It
// forwards to the current kernel's router; a connection is routed once, by
// the kernel current when it arrives.
//
// For a connection to an address it first fills the domain the way the
// kernel's router would from dns.reverse_mapping, falling back to the
// core's shared reverse mapping: a kernel's own mapping starts empty, while
// clients keep addresses the previous kernel answered. The router then skips
// its own lookup (it only looks up an empty Domain); a sniffed name still
// replaces it.
type switchRouter struct {
	current atomic.Pointer[kernel]
	reverse *reversemap.Store
}

func (r *switchRouter) fillDomain(k *kernel, metadata *adapter.InboundContext) {
	// Only where the kernel's router would fill it itself: without
	// reverse_mapping (outside TUN) an address destination has no domain.
	if !k.reverseMapping || metadata.Domain != "" || !metadata.Destination.IsIP() {
		return
	}
	addr := metadata.Destination.Addr
	if dns := service.FromContext[adapter.DNSRouter](k.ctx); dns != nil {
		if domain, ok := dns.LookupReverseMapping(addr); ok {
			metadata.Domain = domain
			return
		}
	}
	if domain, ok := r.reverse.Lookup(addr); ok {
		metadata.Domain = domain
	}
}

var _ adapter.Router = (*switchRouter)(nil)

func (r *switchRouter) set(k *kernel)                           { r.current.Store(k) }
func (r *switchRouter) router() adapter.Router                  { return r.current.Load().Router() }
func (r *switchRouter) Start(adapter.StartStage) error          { return nil }
func (r *switchRouter) Close() error                            { return nil }
func (r *switchRouter) Rules() []adapter.Rule                   { return r.router().Rules() }
func (r *switchRouter) NeedFindProcess() bool                   { return r.router().NeedFindProcess() }
func (r *switchRouter) AppendTracker(adapter.ConnectionTracker) {}
func (r *switchRouter) ResetNetwork()                           { r.router().ResetNetwork() }
func (r *switchRouter) RuleSet(tag string) (adapter.RuleSet, bool) {
	return r.router().RuleSet(tag)
}
func (r *switchRouter) PreMatch(metadata adapter.InboundContext, context tun.DirectRouteContext, timeout time.Duration, supportBypass bool) (tun.DirectRouteDestination, error) {
	return r.router().PreMatch(metadata, context, timeout, supportBypass)
}
func (r *switchRouter) RouteConnection(ctx context.Context, conn net.Conn, metadata adapter.InboundContext) error {
	return r.router().RouteConnection(ctx, conn, metadata)
}
func (r *switchRouter) RoutePacketConnection(ctx context.Context, conn N.PacketConn, metadata adapter.InboundContext) error {
	return r.router().RoutePacketConnection(ctx, conn, metadata)
}
func (r *switchRouter) RouteConnectionEx(ctx context.Context, conn net.Conn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) {
	k := r.current.Load()
	r.fillDomain(k, &metadata)
	k.Router().RouteConnectionEx(ctx, conn, metadata, onClose)
}
func (r *switchRouter) RoutePacketConnectionEx(ctx context.Context, conn N.PacketConn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) {
	k := r.current.Load()
	r.fillDomain(k, &metadata)
	k.Router().RoutePacketConnectionEx(ctx, conn, metadata, onClose)
}
