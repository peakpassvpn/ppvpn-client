package runtime

import (
	"context"
	"time"

	"github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing/service"
)

// dnsWarmUpTimeout bounds the warm-up query; it only has to open the remote
// DNS connection before the first real query needs it.
const dnsWarmUpTimeout = 10 * time.Second

// warmUpRemoteDNS sends one query through the remote DNS server (TUN only)
// so its connection through the selected node is already open when the OS
// sends its first queries. Without it the first queries after start pay the
// node handshake and the resolver's TLS handshake, which over a cross-border
// link outlasts the 1s the Windows resolver waits. ctx is the context the
// instance was created with. Errors are ignored: a real query retries.
func warmUpRemoteDNS(ctx context.Context) {
	manager := service.FromContext[adapter.DNSTransportManager](ctx)
	if manager == nil {
		return
	}
	transport, ok := manager.Transport(config.DNSRemoteTag)
	if !ok {
		return
	}
	ctx, cancel := context.WithTimeout(ctx, dnsWarmUpTimeout)
	defer cancel()
	query := new(dns.Msg)
	query.SetQuestion(".", dns.TypeNS)
	_, _ = transport.Exchange(ctx, query)
}
