// Package outboundlog wraps the node outbounds (Shadowsocks, VLESS, AnyTLS)
// so that, at debug level, a failed connection to a node writes one line to
// the first-party diagnostic log: which node and ingress, the protocol, the
// destination asked for, the underlying error and how long it took.
//
// sing-box's own log stays disabled (its text is not covered by the
// credential policy), so without this a failed handshake or authentication
// was only visible as a generic PROXY_REQUEST_FAILED. Two failures are
// logged:
//
//   - the dial fails ("dial"): TCP connect, and for VLESS/REALITY and
//     AnyTLS also the TLS handshake, which runs while dialing;
//   - the node closes the connection before sending a single byte
//     ("closed before any response"): Shadowsocks sends its request lazily,
//     so a rejected key only shows on the first read.
//
// The types are registered under their usual names, so options and golden
// files are unchanged.
package outboundlog

import (
	"context"
	"errors"
	"io"
	"net"
	"sync/atomic"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing-box/adapter/outbound"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/log"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing-box/protocol/anytls"
	"github.com/sagernet/sing-box/protocol/shadowsocks"
	"github.com/sagernet/sing-box/protocol/vless"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
)

// Ingress names the node and ingress an outbound tag belongs to.
type Ingress struct {
	NodeID      string
	EndpointKey string
}

type contextKey struct{}

type settings struct {
	log       *corelog.Logger
	ingresses map[string]Ingress
}

// WithLogger makes the node outbounds created under ctx log failures to l,
// naming each outbound tag's node and ingress from ingresses.
func WithLogger(ctx context.Context, l *corelog.Logger, ingresses map[string]Ingress) context.Context {
	return context.WithValue(ctx, contextKey{}, settings{log: l, ingresses: ingresses})
}

// Register replaces the node protocol constructors with wrapping ones.
func Register(registry *outbound.Registry) {
	outbound.Register[option.ShadowsocksOutboundOptions](registry, C.TypeShadowsocks, wrap(shadowsocks.NewOutbound))
	outbound.Register[option.VLESSOutboundOptions](registry, C.TypeVLESS, wrap(vless.NewOutbound))
	outbound.Register[option.AnyTLSOutboundOptions](registry, C.TypeAnyTLS, wrap(anytls.NewOutbound))
}

func wrap[T any](inner outbound.ConstructorFunc[T]) outbound.ConstructorFunc[T] {
	return func(ctx context.Context, router adapter.Router, logger log.ContextLogger, tag string, options T) (adapter.Outbound, error) {
		out, err := inner(ctx, router, logger, tag, options)
		if err != nil {
			return nil, err
		}
		s, ok := ctx.Value(contextKey{}).(settings)
		if !ok || s.log == nil {
			return out, nil
		}
		return &logged{Outbound: out, log: s.log, ingress: s.ingresses[tag]}, nil
	}
}

// logged forwards everything to the node outbound, including its lifecycle
// and interface-change hooks, and never alters a result.
type logged struct {
	adapter.Outbound
	log     *corelog.Logger
	ingress Ingress
}

func (l *logged) Start(stage adapter.StartStage) error {
	return adapter.LegacyStart(l.Outbound, stage)
}

func (l *logged) Close() error {
	if closer, ok := l.Outbound.(io.Closer); ok {
		return closer.Close()
	}
	return nil
}

func (l *logged) InterfaceUpdated() {
	if listener, ok := l.Outbound.(interface{ InterfaceUpdated() }); ok {
		listener.InterfaceUpdated()
	}
}

func (l *logged) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	started := time.Now()
	conn, err := l.Outbound.DialContext(ctx, network, destination)
	if err != nil {
		l.failed("dial", network, destination, err, started)
		return nil, err
	}
	if !l.log.DebugEnabled() {
		return conn, nil
	}
	return &firstReadConn{Conn: conn, report: func(err error) {
		l.failed("closed before any response", network, destination, err, started)
	}}, nil
}

func (l *logged) ListenPacket(ctx context.Context, destination M.Socksaddr) (net.PacketConn, error) {
	started := time.Now()
	conn, err := l.Outbound.ListenPacket(ctx, destination)
	if err != nil {
		l.failed("dial", N.NetworkUDP, destination, err, started)
	}
	return conn, err
}

func (l *logged) failed(stage, network string, destination M.Socksaddr, err error, started time.Time) {
	if !l.log.DebugEnabled() || errors.Is(err, context.Canceled) {
		return
	}
	l.log.Debug("outbound failed", "stage", stage, "node_id", l.ingress.NodeID, "endpoint_key", l.ingress.EndpointKey,
		"outbound", l.Tag(), "protocol", l.Type(), "network", network, "destination", destination.String(),
		"error", err, "ms", time.Since(started).Milliseconds())
}

// firstReadConn reports a read error that ends the connection before any
// byte arrived (the node rejected or dropped it). Only the reading goroutine
// touches received.
type firstReadConn struct {
	net.Conn
	received bool
	reported atomic.Bool
	report   func(error)
}

func (c *firstReadConn) Read(p []byte) (int, error) {
	n, err := c.Conn.Read(p)
	if n > 0 {
		c.received = true
	}
	if err != nil && !c.received && c.reported.CompareAndSwap(false, true) {
		c.report(err)
	}
	return n, err
}

func (c *firstReadConn) Upstream() any { return c.Conn }
