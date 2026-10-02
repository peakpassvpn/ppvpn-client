package outboundlog

import (
	"context"
	"errors"
	"net"
	"net/netip"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/log"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing-box/protocol/direct"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
)

// DirectLimit is how often one direct outbound logs a failure to the same
// destination: while the network is down every direct connection fails, so
// the first failure is logged and the next ones within DirectLimit only
// counted (suppressed= on the next line). A variable for tests, read when
// an outbound is created.
var DirectLimit = 10 * time.Second

// directLimiterSize bounds the destinations remembered; past it, entries
// older than the window are dropped.
const directLimiterSize = 1024

func wrapDirect(ctx context.Context, router adapter.Router, logger log.ContextLogger, tag string, options option.DirectOutboundOptions) (adapter.Outbound, error) {
	out, err := direct.NewOutbound(ctx, router, logger, tag, options)
	if err != nil {
		return nil, err
	}
	s, ok := ctx.Value(contextKey{}).(settings)
	inner, isDirect := out.(*direct.Outbound)
	if !ok || s.log == nil || !isDirect {
		return out, nil
	}
	return &loggedDirect{Outbound: inner, log: s.log, limiter: newLimiter(DirectLimit)}, nil
}

// loggedDirect embeds the direct outbound, so sing-box still finds its
// parallel and network-strategy dialers, ICMP routing and IsEmpty, and logs
// a failure of every way it dials.
type loggedDirect struct {
	*direct.Outbound
	log     *corelog.Logger
	limiter *limiter
}

func (d *loggedDirect) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	started := time.Now()
	conn, err := d.Outbound.DialContext(ctx, network, destination)
	if err != nil {
		d.failed(network, destination, err, started)
	}
	return conn, err
}

func (d *loggedDirect) ListenPacket(ctx context.Context, destination M.Socksaddr) (net.PacketConn, error) {
	started := time.Now()
	conn, err := d.Outbound.ListenPacket(ctx, destination)
	if err != nil {
		d.failed(N.NetworkUDP, destination, err, started)
	}
	return conn, err
}

func (d *loggedDirect) DialParallel(ctx context.Context, network string, destination M.Socksaddr, destinationAddresses []netip.Addr) (net.Conn, error) {
	started := time.Now()
	conn, err := d.Outbound.DialParallel(ctx, network, destination, destinationAddresses)
	if err != nil {
		d.failed(network, destination, err, started)
	}
	return conn, err
}

func (d *loggedDirect) DialParallelNetwork(ctx context.Context, network string, destination M.Socksaddr, destinationAddresses []netip.Addr, networkStrategy *C.NetworkStrategy, networkType []C.InterfaceType, fallbackNetworkType []C.InterfaceType, fallbackDelay time.Duration) (net.Conn, error) {
	started := time.Now()
	conn, err := d.Outbound.DialParallelNetwork(ctx, network, destination, destinationAddresses, networkStrategy, networkType, fallbackNetworkType, fallbackDelay)
	if err != nil {
		d.failed(network, destination, err, started)
	}
	return conn, err
}

func (d *loggedDirect) ListenSerialNetworkPacket(ctx context.Context, destination M.Socksaddr, destinationAddresses []netip.Addr, networkStrategy *C.NetworkStrategy, networkType []C.InterfaceType, fallbackNetworkType []C.InterfaceType, fallbackDelay time.Duration) (net.PacketConn, netip.Addr, error) {
	started := time.Now()
	conn, address, err := d.Outbound.ListenSerialNetworkPacket(ctx, destination, destinationAddresses, networkStrategy, networkType, fallbackNetworkType, fallbackDelay)
	if err != nil {
		d.failed(N.NetworkUDP, destination, err, started)
	}
	return conn, address, err
}

// failed writes the node outbounds' "outbound failed" line (no node_id or
// endpoint_key: a direct connection has none), at most once per
// destination per DirectLimit.
func (d *loggedDirect) failed(network string, destination M.Socksaddr, err error, started time.Time) {
	if !d.log.DebugEnabled() || errors.Is(err, context.Canceled) {
		return
	}
	network = N.NetworkName(network)
	key := network + " " + destination.String()
	log, suppressed := d.limiter.allow(key, time.Now())
	if !log {
		return
	}
	fields := []any{"stage", "dial", "outbound", d.Tag(), "protocol", d.Type(), "network", network,
		"destination", destination.String(), "error", err, "ms", time.Since(started).Milliseconds()}
	if suppressed > 0 {
		fields = append(fields, "suppressed", suppressed)
	}
	d.log.Debug("outbound failed", fields...)
}

type limitEntry struct {
	logged     time.Time
	suppressed int
}

type limiter struct {
	window time.Duration
	mu     sync.Mutex
	seen   map[string]*limitEntry
}

func newLimiter(window time.Duration) *limiter {
	return &limiter{window: window, seen: make(map[string]*limitEntry)}
}

// allow reports whether a failure for key is logged now and, if so, how
// many were suppressed since the last one logged.
func (l *limiter) allow(key string, now time.Time) (bool, int) {
	l.mu.Lock()
	defer l.mu.Unlock()
	entry, ok := l.seen[key]
	if ok && now.Sub(entry.logged) < l.window {
		entry.suppressed++
		return false, 0
	}
	if !ok {
		if len(l.seen) >= directLimiterSize {
			for k, e := range l.seen {
				if now.Sub(e.logged) >= l.window {
					delete(l.seen, k)
				}
			}
		}
		entry = &limitEntry{}
		l.seen[key] = entry
	}
	suppressed := entry.suppressed
	entry.logged, entry.suppressed = now, 0
	return true, suppressed
}
