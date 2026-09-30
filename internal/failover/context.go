package failover

import (
	"context"

	"github.com/peakpassvpn/ppvpn-core/internal/dnslog"
	"github.com/peakpassvpn/ppvpn-core/internal/domaindest"
	"github.com/peakpassvpn/ppvpn-core/internal/proxyinbound"
	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/include"
)

// Context returns a sing-box context whose registries contain every upstream
// protocol plus the ppvpn failover group, the TUN domain-destination wrapper, the
// shared local proxy inbound and debug logging around the DNS transports.
// Every place that builds, decodes or encodes sing-box options for this core
// must use it instead of include.Context.
func Context(ctx context.Context) context.Context {
	outbounds := include.OutboundRegistry()
	Register(outbounds)
	domaindest.Register(outbounds)
	inbounds := include.InboundRegistry()
	proxyinbound.Register(inbounds)
	transports := include.DNSTransportRegistry()
	dnslog.Register(transports)
	return box.Context(ctx, inbounds, outbounds, include.EndpointRegistry(), transports, include.ServiceRegistry())
}
