// Package domaindest hands the domain of a TUN connection to the proxy.
//
// A TUN inbound only sees IP destinations. sing-box 1.13 records the domain
// it learns for such a connection in InboundContext.Domain (from the sniff
// rule action: TLS SNI, HTTP Host, QUIC; or from dns.reverse_mapping for an
// address its own DNS module answered), but it no longer rewrites the
// destination with it: sniff_override_destination was deprecated in 1.11 and
// nothing in 1.13 sets RuleActionSniff.OverrideDestination any more. Without
// a rewrite a proxy node receives the address a local resolver chose, which
// may be poisoned, geo-mismatched or a LAN gateway's fake-ip (198.18.0.0/15)
// that the node cannot reach.
//
// This outbound wraps a proxy outbound (the node selector or one node). For a
// connection from one of the configured inbounds whose destination is an IP
// and whose domain is known, it replaces the destination with that domain
// before handing the connection to the wrapped outbound, exactly like a
// route-options override_address to a domain does in route.matchRule: the
// original address is kept in RouteOriginalDestination, from which the
// connection manager maps UDP replies back. The node then resolves the domain
// remotely. Every other connection passes through untouched.
package domaindest

import (
	"context"
	"net"
	"net/netip"
	"slices"

	"github.com/peakpassvpn/ppvpn-core/internal/reversemap"
	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing-box/adapter/outbound"
	"github.com/sagernet/sing-box/log"
	E "github.com/sagernet/sing/common/exceptions"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/service"
)

// Type is the sing-box outbound type registered by Register.
const Type = "ppvpn-domain-destination"

// Options configures one wrapper. Inbounds lists the inbound tags whose
// connections are rewritten; it must not be empty.
type Options struct {
	Outbound string   `json:"outbound"`
	Inbounds []string `json:"inbounds"`
	// IPv6Only rewrites only connections to a global unicast IPv6 address.
	IPv6Only bool `json:"ipv6_only,omitempty"`
}

// Register adds the outbound type to a sing-box outbound registry.
func Register(registry *outbound.Registry) {
	outbound.Register[Options](registry, Type, New)
}

type Outbound struct {
	outbound.Adapter
	manager    adapter.OutboundManager
	connection adapter.ConnectionManager
	dns        adapter.DNSRouter
	// reverse is the core's reverse mapping shared across kernels, read
	// after dns (whose own mapping starts empty in every new kernel).
	reverse   *reversemap.Store
	targetTag string
	inbounds  []string
	ipv6Only  bool
	target    adapter.Outbound
}

var (
	_ adapter.OutboundGroup             = (*Outbound)(nil)
	_ adapter.ConnectionHandlerEx       = (*Outbound)(nil)
	_ adapter.PacketConnectionHandlerEx = (*Outbound)(nil)
	_ adapter.Lifecycle                 = (*Outbound)(nil)
)

func New(ctx context.Context, _ adapter.Router, _ log.ContextLogger, tag string, options Options) (adapter.Outbound, error) {
	if options.Outbound == "" || options.Outbound == tag {
		return nil, E.New("invalid wrapped outbound")
	}
	if len(options.Inbounds) == 0 {
		return nil, E.New("missing inbounds")
	}
	return &Outbound{
		Adapter:    outbound.NewAdapter(Type, tag, []string{N.NetworkTCP, N.NetworkUDP}, []string{options.Outbound}),
		manager:    service.FromContext[adapter.OutboundManager](ctx),
		connection: service.FromContext[adapter.ConnectionManager](ctx),
		dns:        service.FromContext[adapter.DNSRouter](ctx),
		reverse:    reversemap.FromContext(ctx),
		targetTag:  options.Outbound,
		inbounds:   slices.Clone(options.Inbounds),
		ipv6Only:   options.IPv6Only,
	}, nil
}

func (o *Outbound) Start(stage adapter.StartStage) error {
	if stage != adapter.StartStateStart {
		return nil
	}
	if o.manager == nil || o.connection == nil {
		return E.New("missing outbound or connection manager")
	}
	target, ok := o.manager.Outbound(o.targetTag)
	if !ok {
		return E.New("wrapped outbound not found: ", o.targetTag)
	}
	o.target = target
	return nil
}

func (o *Outbound) Close() error { return nil }

// Network reports what the wrapped outbound supports.
func (o *Outbound) Network() []string {
	if o.target != nil {
		return o.target.Network()
	}
	return o.Adapter.Network()
}

// Now reports the outbound that actually carries traffic, so connection
// telemetry attributes a wrapped connection exactly as it would attribute the
// wrapped outbound (a selector reports its selected node).
func (o *Outbound) Now() string {
	if group, ok := o.target.(adapter.OutboundGroup); ok {
		return group.Now()
	}
	return o.targetTag
}

func (o *Outbound) All() []string { return []string{o.targetTag} }

func (o *Outbound) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	return o.target.DialContext(ctx, network, destination)
}

func (o *Outbound) ListenPacket(ctx context.Context, destination M.Socksaddr) (net.PacketConn, error) {
	return o.target.ListenPacket(ctx, destination)
}

func (o *Outbound) NewConnectionEx(ctx context.Context, conn net.Conn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) {
	o.Restore(&metadata)
	if handler, ok := o.target.(adapter.ConnectionHandlerEx); ok {
		handler.NewConnectionEx(ctx, conn, metadata, onClose)
		return
	}
	o.connection.NewConnection(ctx, o.target, conn, metadata, onClose)
}

func (o *Outbound) NewPacketConnectionEx(ctx context.Context, conn N.PacketConn, metadata adapter.InboundContext, onClose N.CloseHandlerFunc) {
	o.Restore(&metadata)
	if handler, ok := o.target.(adapter.PacketConnectionHandlerEx); ok {
		handler.NewPacketConnectionEx(ctx, conn, metadata, onClose)
		return
	}
	o.connection.NewPacketConnection(ctx, o.target, conn, metadata, onClose)
}

// Restore replaces an IP destination with the known domain for connections
// from the configured inbounds. It reports whether it rewrote metadata.
//
// The sniffed domain wins. The router fills Domain from dns.reverse_mapping
// before sniffing, but a sniffer that finds no name (a TLS ClientHello
// without SNI, an HTTP Host that is an address) overwrites it, so the reverse
// mapping is consulted again here.
func (o *Outbound) Restore(metadata *adapter.InboundContext) bool {
	if !metadata.Destination.IsIP() || !slices.Contains(o.inbounds, metadata.Inbound) {
		return false
	}
	if o.ipv6Only && !globalIPv6(metadata.Destination.Addr) {
		return false
	}
	domain := metadata.Domain
	if !isDomain(domain) {
		domain = ""
		if o.dns != nil {
			domain, _ = o.dns.LookupReverseMapping(metadata.Destination.Addr)
		}
		if !isDomain(domain) {
			domain, _ = o.reverse.Lookup(metadata.Destination.Addr)
		}
		if !isDomain(domain) {
			return false
		}
	}
	if !metadata.RouteOriginalDestination.IsValid() {
		metadata.RouteOriginalDestination = metadata.Destination
	}
	metadata.Destination = M.Socksaddr{Fqdn: domain, Port: metadata.Destination.Port}
	metadata.DestinationAddresses = nil
	return true
}

// globalIPv6 reports whether a is a global unicast IPv6 address
// (2000::/3): not IPv4, ULA, link-local or any other local scope.
func globalIPv6(a netip.Addr) bool {
	return a.Is6() && !a.Is4In6() && a.As16()[0]&0xe0 == 0x20
}

func isDomain(value string) bool {
	if !M.IsDomainName(value) {
		return false
	}
	_, err := netip.ParseAddr(value)
	return err != nil
}
