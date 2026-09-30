package runtime

import (
	"context"
	"fmt"
	"net"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/option"
	M "github.com/sagernet/sing/common/metadata"
)

type engine interface {
	Start() error
	Close() error
}
type telemetryEngine interface {
	telemetrySnapshot() (Traffic, []Connection)
}
type flowEngine interface {
	dialFlow(ctx context.Context, network, outboundTag, host string, port uint16) (net.Conn, error)
	selectOutbound(outboundTag string) bool
}

// ingressEngine reports which ingress a node outbound is using.
type ingressEngine interface {
	activeIngress(nodeTag string) (failover.Active, bool)
}

// inboundEngine can open and close one listener without restarting, so
// toggling the system proxy never drops other connections.
type inboundEngine interface {
	addInbound(inbound option.Inbound) error
	removeInbound(tag string) error
}

// directEngine dials through the instance's "direct" outbound.
type directEngine interface {
	dialDirect(ctx context.Context, network, address string) (net.Conn, error)
}

// dnsWarmEngine opens the remote DNS connection ahead of the first query
// (TUN only; see warmUpRemoteDNS). It returns at once.
type dnsWarmEngine interface {
	warmUpDNS()
}

// connectionLogEngine writes a debug line per routed connection.
type connectionLogEngine interface {
	setConnectionLog(log *corelog.Logger)
}

type singEngine struct {
	*box.Box
	ctx     context.Context
	tracker *telemetry
}

func (e *singEngine) activeIngress(nodeTag string) (failover.Active, bool) {
	outbound, ok := e.Outbound().Outbound(nodeTag)
	if !ok {
		return failover.Active{}, false
	}
	if group, ok := outbound.(*failover.Group); ok {
		return group.Active(), true
	}
	// A single-ingress node is the ingress outbound itself.
	return failover.Active{Current: nodeTag}, true
}
func (e *singEngine) addInbound(inbound option.Inbound) error {
	return e.Inbound().Create(e.ctx, e.Router(), e.LogFactory().NewLogger("inbound/"+inbound.Tag), inbound.Tag, inbound.Type, inbound.Options)
}
func (e *singEngine) removeInbound(tag string) error {
	if _, ok := e.Inbound().Get(tag); !ok {
		return nil
	}
	return e.Inbound().Remove(tag)
}

func (e *singEngine) warmUpDNS() { go warmUpRemoteDNS(e.ctx) }

func (e *singEngine) setConnectionLog(log *corelog.Logger) { e.tracker.log.Store(log) }

func (e *singEngine) telemetrySnapshot() (Traffic, []Connection) { return e.tracker.snapshot() }
func (e *singEngine) dialFlow(ctx context.Context, network, outboundTag, host string, port uint16) (net.Conn, error) {
	outbound, ok := e.Outbound().Outbound(outboundTag)
	if !ok {
		return nil, fmt.Errorf("outbound not found")
	}
	return outbound.DialContext(ctx, network, M.ParseSocksaddrHostPort(host, port))
}
func (e *singEngine) dialDirect(ctx context.Context, network, address string) (net.Conn, error) {
	outbound, ok := e.Outbound().Outbound("direct")
	if !ok {
		return nil, fmt.Errorf("direct outbound not found")
	}
	return outbound.DialContext(ctx, network, M.ParseSocksaddr(address))
}
func (e *singEngine) selectOutbound(outboundTag string) bool {
	outbound, ok := e.Outbound().Outbound("selected")
	if !ok {
		return false
	}
	selector, ok := outbound.(interface{ SelectOutbound(string) bool })
	return ok && selector.SelectOutbound(outboundTag)
}

type engineFactory func(context.Context, option.Options) (engine, error)

func newSingBox(ctx context.Context, options option.Options) (engine, error) {
	// box.New registers its services in the registry carried by this
	// context, so the same context can create inbounds later.
	ctx = failover.Context(ctx)
	instance, err := box.New(box.Options{Context: ctx, Options: options})
	if err != nil {
		return nil, err
	}
	tracker := newTelemetry()
	instance.Router().AppendTracker(tracker)
	return &singEngine{Box: instance, ctx: ctx, tracker: tracker}, nil
}
