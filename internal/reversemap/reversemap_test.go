package reversemap

import (
	"net"
	"net/netip"
	"testing"
	"time"

	mDNS "github.com/miekg/dns"
)

func answer(name string, ttl uint32, ips ...string) *mDNS.Msg {
	response := new(mDNS.Msg)
	for _, ip := range ips {
		parsed := net.ParseIP(ip)
		header := mDNS.RR_Header{Name: name, Class: mDNS.ClassINET, Ttl: ttl}
		if parsed.To4() != nil {
			header.Rrtype = mDNS.TypeA
			response.Answer = append(response.Answer, &mDNS.A{Hdr: header, A: parsed})
		} else {
			header.Rrtype = mDNS.TypeAAAA
			response.Answer = append(response.Answer, &mDNS.AAAA{Hdr: header, AAAA: parsed})
		}
	}
	return response
}

func TestRecordLookupAndExpiry(t *testing.T) {
	now := time.Unix(1000, 0)
	s := New()
	s.now = func() time.Time { return now }
	s.Record(answer("example.com.", 60, "203.0.113.7", "2001:db8::7"))
	for _, ip := range []string{"203.0.113.7", "2001:db8::7", "::ffff:203.0.113.7"} {
		if domain, ok := s.Lookup(netip.MustParseAddr(ip)); !ok || domain != "example.com" {
			t.Fatalf("%s: %q %v", ip, domain, ok)
		}
	}
	now = now.Add(61 * time.Second)
	if _, ok := s.Lookup(netip.MustParseAddr("203.0.113.7")); ok {
		t.Fatal("expired entry returned")
	}
	// Zero TTL and non-address records are ignored; nil is safe.
	s.Record(answer("zero.example.", 0, "203.0.113.8"))
	if _, ok := s.Lookup(netip.MustParseAddr("203.0.113.8")); ok {
		t.Fatal("zero-TTL answer recorded")
	}
	var none *Store
	none.Record(answer("x.", 60, "203.0.113.9"))
	if _, ok := none.Lookup(netip.MustParseAddr("203.0.113.9")); ok {
		t.Fatal("nil store found something")
	}
}

func TestCapacityEvictsTheEntryClosestToExpiry(t *testing.T) {
	now := time.Unix(1000, 0)
	s := New()
	s.now = func() time.Time { return now }
	s.Record(answer("short.example.", 10, "10.0.0.1"))
	for i := 2; i <= Capacity; i++ {
		addr := netip.AddrFrom4([4]byte{10, byte(i >> 16), byte(i >> 8), byte(i)})
		s.Record(answer("long.example.", 3600, addr.String()))
	}
	s.Record(answer("new.example.", 3600, "192.0.2.1"))
	if _, ok := s.Lookup(netip.MustParseAddr("10.0.0.1")); ok {
		t.Fatal("entry closest to expiry kept over capacity")
	}
	if _, ok := s.Lookup(netip.MustParseAddr("192.0.2.1")); !ok || len(s.entries) > Capacity {
		t.Fatalf("new entry missing or over capacity: %d", len(s.entries))
	}
}
