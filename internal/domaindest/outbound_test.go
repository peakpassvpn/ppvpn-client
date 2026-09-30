package domaindest

import (
	"net/netip"
	"testing"

	"github.com/sagernet/sing-box/adapter"
	M "github.com/sagernet/sing/common/metadata"
)

type reverseMapping struct {
	adapter.DNSRouter
	table map[netip.Addr]string
}

func (r reverseMapping) LookupReverseMapping(ip netip.Addr) (string, bool) {
	domain, ok := r.table[ip]
	return domain, ok
}

func TestRestore(t *testing.T) {
	o := &Outbound{inbounds: []string{"tun"}, dns: reverseMapping{table: map[netip.Addr]string{
		netip.MustParseAddr("203.0.113.7"): "mapped.example",
	}}}
	ip := M.ParseSocksaddr("203.0.113.9:443")
	mapped := M.ParseSocksaddr("203.0.113.7:443")
	for _, tc := range []struct {
		name     string
		metadata adapter.InboundContext
		want     string
	}{
		{"sniffed", adapter.InboundContext{Inbound: "tun", Destination: ip, Domain: "sni.example"}, "sni.example:443"},
		{"sniffed wins over mapping", adapter.InboundContext{Inbound: "tun", Destination: mapped, Domain: "sni.example"}, "sni.example:443"},
		{"reverse mapping", adapter.InboundContext{Inbound: "tun", Destination: mapped}, "mapped.example:443"},
		{"host is an address", adapter.InboundContext{Inbound: "tun", Destination: mapped, Domain: "203.0.113.7"}, "mapped.example:443"},
		{"unknown", adapter.InboundContext{Inbound: "tun", Destination: ip}, "203.0.113.9:443"},
		{"other inbound", adapter.InboundContext{Inbound: "system-proxy", Destination: ip, Domain: "sni.example"}, "203.0.113.9:443"},
		{"already a domain", adapter.InboundContext{Inbound: "tun", Destination: M.ParseSocksaddr("asked.example:80"), Domain: "sni.example"}, "asked.example:80"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			metadata := tc.metadata
			rewritten := o.Restore(&metadata)
			if got := metadata.Destination.String(); got != tc.want {
				t.Fatalf("destination %s, want %s", got, tc.want)
			}
			if rewritten != (metadata.Destination != tc.metadata.Destination) {
				t.Fatalf("reported %v", rewritten)
			}
			if rewritten && metadata.RouteOriginalDestination != tc.metadata.Destination {
				t.Fatalf("original destination lost: %s", metadata.RouteOriginalDestination)
			}
		})
	}
}
