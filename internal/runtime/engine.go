package runtime

import (
	"context"
	"fmt"
	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing/common/control"
	"github.com/sagernet/sing/service"
	"net"
	"strings"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/internal/tunrules"
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

// interfaceWatchEngine logs the default interface sing-box binds outbound
// sockets to (auto_detect_interface), at start and on every change, and calls
// changed (when not nil) after each change.
type interfaceWatchEngine interface {
	// changed gets the new default interface (nil: none).
	watchDefaultInterface(log *corelog.Logger, changed func(*control.Interface))
}

// networkStateEngine reports whether a default interface exists. known is
// false without an interface monitor (no auto_detect_interface), when the
// engine cannot tell.
type networkStateEngine interface {
	defaultInterfaceState() (known, present bool)
}

// tunRoutingEngine keeps the TUN's policy routing rules in place (Linux;
// see tunrules). setTUNRouting is called before Start.
type tunRoutingEngine interface {
	setTUNRouting(log *corelog.Logger, changed func(tunrules.State))
	tunRouting() string
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

func (e *singEngine) ingressGroup(nodeTag string) (*failover.Group, bool) {
	outbound, ok := e.Outbound().Outbound(nodeTag)
	if !ok {
		return nil, false
	}
	group, ok := outbound.(*failover.Group)
	return group, ok
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

func (e *singEngine) setConnectionLog(log *corelog.Logger) { e.tracker.log.Store(log) }

func (e *singEngine) defaultInterfaceState() (known, present bool) {
	manager := service.FromContext[adapter.NetworkManager](e.ctx)
	if manager == nil || manager.InterfaceMonitor() == nil {
		return false, false
	}
	return true, manager.InterfaceMonitor().DefaultInterface() != nil
}

func (e *singEngine) watchDefaultInterface(log *corelog.Logger, changed func(*control.Interface)) {
	manager := service.FromContext[adapter.NetworkManager](e.ctx)
	if manager == nil {
		return
	}
	monitor := manager.InterfaceMonitor()
	if monitor == nil {
		// No auto_detect_interface: outbound sockets are not bound.
		return
	}
	logDefaultInterface(log, "start", monitor.DefaultInterface())
	monitor.RegisterCallback(func(defaultInterface *control.Interface, _ int) {
		logDefaultInterface(log, "changed", defaultInterface)
		if changed != nil {
			changed(defaultInterface)
		}
	})
}

// logDefaultInterface writes one info line naming the interface outbound
// sockets are bound to; a nil interface means none was found (no network).
func logDefaultInterface(log *corelog.Logger, event string, defaultInterface *control.Interface) {
	if defaultInterface == nil {
		log.Info("default interface", "event", event, "name", "none")
		return
	}
	addresses := make([]string, len(defaultInterface.Addresses))
	for i, prefix := range defaultInterface.Addresses {
		addresses[i] = prefix.String()
	}
	log.Info("default interface", "event", event, "name", defaultInterface.Name, "index", defaultInterface.Index,
		"mtu", defaultInterface.MTU, "addresses", strings.Join(addresses, ","))
}

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

// newSingBox is the production engine: a layered engine (layered.go), whose
// kernel can be replaced without closing listeners.
func newSingBox(ctx context.Context, options option.Options) (engine, error) {
	return newLayeredEngine(ctx, options)
}
