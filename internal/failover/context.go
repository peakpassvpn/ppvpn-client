package failover

import (
	"context"

	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/include"
)

// Context returns a sing-box context whose registries contain every upstream
// protocol plus the ppvpn failover group. Every place that builds, decodes or
// encodes sing-box options for this core must use it instead of
// include.Context.
func Context(ctx context.Context) context.Context {
	outbounds := include.OutboundRegistry()
	Register(outbounds)
	return box.Context(ctx, include.InboundRegistry(), outbounds, include.EndpointRegistry(), include.DNSTransportRegistry(), include.ServiceRegistry())
}
