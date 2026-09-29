package runtime

import (
	"context"
	"fmt"
	"net"

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
type singEngine struct {
	*box.Box
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
func (e *singEngine) telemetrySnapshot() (Traffic, []Connection) { return e.tracker.snapshot() }
func (e *singEngine) dialFlow(ctx context.Context, network, outboundTag, host string, port uint16) (net.Conn, error) {
	outbound, ok := e.Outbound().Outbound(outboundTag)
	if !ok {
		return nil, fmt.Errorf("outbound not found")
	}
	return outbound.DialContext(ctx, network, M.ParseSocksaddrHostPort(host, port))
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
	instance, err := box.New(box.Options{Context: failover.Context(ctx), Options: options})
	if err != nil {
		return nil, err
	}
	tracker := newTelemetry()
	instance.Router().AppendTracker(tracker)
	return &singEngine{Box: instance, tracker: tracker}, nil
}
