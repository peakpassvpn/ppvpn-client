package domaindest

import (
	"net"
	"net/netip"
	"testing"

	mDNS "github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/reversemap"
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

func TestRestoreIPv6Only(t *testing.T) {
	o := &Outbound{inbounds: []string{"tun"}, ipv6Only: true, dns: reverseMapping{table: map[netip.Addr]string{
		netip.MustParseAddr("2001:db8::50"): "dual.example",
	}}}
	for _, tc := range []struct {
		name        string
		destination string
		domain      string
		want        string
	}{
		{"global IPv6, sniffed", "[2001:db8::7]:443", "sni.example", "sni.example:443"},
		{"global IPv6, reverse mapping", "[2001:db8::50]:9999", "", "dual.example:9999"},
		{"global IPv6, unknown", "[2001:db8::9]:443", "", "[2001:db8::9]:443"},
		{"IPv4 untouched", "203.0.113.9:443", "sni.example", "203.0.113.9:443"},
		{"ULA untouched", "[fd00::1]:443", "sni.example", "[fd00::1]:443"},
		{"link-local untouched", "[fe80::1]:443", "sni.example", "[fe80::1]:443"},
		{"IPv4-mapped untouched", "[::ffff:203.0.113.9]:443", "sni.example", "[::ffff:203.0.113.9]:443"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			metadata := adapter.InboundContext{Inbound: "tun", Destination: M.ParseSocksaddr(tc.destination), Domain: tc.domain}
			o.Restore(&metadata)
			if got := metadata.Destination.String(); got != tc.want {
				t.Fatalf("destination %s, want %s", got, tc.want)
			}
		})
	}
}

// After a kernel switch the new kernel's own reverse mapping is empty; the
// shared store still gives direct's ipv6_only wrapper (no host IPv6 path)
// the domain of a global IPv6 address, so it goes out by IPv4. Other
// destinations stay untouched.
func TestRestoreFallsBackToTheSharedReverseMapping(t *testing.T) {
	store := reversemap.New()
	answer := new(mDNS.Msg)
	answer.Answer = append(answer.Answer, &mDNS.AAAA{Hdr: mDNS.RR_Header{Name: "dual.example.", Rrtype: mDNS.TypeAAAA, Class: mDNS.ClassINET, Ttl: 60}, AAAA: net.ParseIP("2001:db8::50")})
	answer.Answer = append(answer.Answer, &mDNS.A{Hdr: mDNS.RR_Header{Name: "dual.example.", Rrtype: mDNS.TypeA, Class: mDNS.ClassINET, Ttl: 60}, A: net.ParseIP("203.0.113.50")})
	store.Record(answer)
	o := &Outbound{inbounds: []string{"tun"}, ipv6Only: true, dns: reverseMapping{table: map[netip.Addr]string{}}, reverse: store}
	for destination, want := range map[string]string{
		"[2001:db8::50]:443": "dual.example:443",
		"203.0.113.50:443":   "203.0.113.50:443", // ipv6_only: IPv4 untouched
		"[2001:db8::51]:443": "[2001:db8::51]:443",
	} {
		metadata := adapter.InboundContext{Inbound: "tun", Destination: M.ParseSocksaddr(destination)}
		o.Restore(&metadata)
		if got := metadata.Destination.String(); got != want {
			t.Errorf("%s: %s, want %s", destination, got, want)
		}
	}
}
