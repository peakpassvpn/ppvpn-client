// Package reversemap keeps the address → domain answers the core's DNS
// servers returned, for as long as their TTL, independently of any one
// sing-box instance.
//
// sing-box's own dns.reverse_mapping lives in its DNS router, so it starts
// empty in every new kernel. When an apply-profile switches kernels, clients
// still hold addresses resolved by the old one (until their TTL), and a new
// connection to such an address would lose its domain: domain route rules
// would not match and domaindest would hand the node an IP. The core records
// every A/AAAA answer here and consults this store after the kernel's own
// mapping.
package reversemap

import (
	"context"
	"net/netip"
	"strings"
	"sync"
	"time"

	mDNS "github.com/miekg/dns"
)

// Capacity bounds the store; the entry closest to expiry is evicted first.
const Capacity = 8192

// Store maps an address to the domain it was last answered for. The zero
// value is not usable; use New. A nil *Store records and finds nothing.
type Store struct {
	mu      sync.Mutex
	entries map[netip.Addr]entry
	now     func() time.Time
}

type entry struct {
	domain  string
	expires time.Time
}

func New() *Store { return &Store{entries: map[netip.Addr]entry{}, now: time.Now} }

// Record stores the A and AAAA answers of response under their owner name
// (as sing-box's reverse mapping does), each for its TTL.
func (s *Store) Record(response *mDNS.Msg) {
	if s == nil || response == nil {
		return
	}
	now := s.now()
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, answer := range response.Answer {
		var addr netip.Addr
		var ok bool
		switch record := answer.(type) {
		case *mDNS.A:
			addr, ok = netip.AddrFromSlice(record.A.To4())
		case *mDNS.AAAA:
			addr, ok = netip.AddrFromSlice(record.AAAA.To16())
		default:
			continue
		}
		ttl := time.Duration(answer.Header().Ttl) * time.Second
		if !ok || ttl <= 0 {
			continue
		}
		if _, exists := s.entries[addr]; !exists && len(s.entries) >= Capacity {
			s.evictLocked(now)
		}
		s.entries[addr] = entry{domain: strings.TrimSuffix(answer.Header().Name, "."), expires: now.Add(ttl)}
	}
}

// evictLocked drops expired entries, or else the one closest to expiry.
func (s *Store) evictLocked(now time.Time) {
	var oldest netip.Addr
	var oldestAt time.Time
	for addr, e := range s.entries {
		if !now.Before(e.expires) {
			delete(s.entries, addr)
			continue
		}
		if oldestAt.IsZero() || e.expires.Before(oldestAt) {
			oldest, oldestAt = addr, e.expires
		}
	}
	if len(s.entries) >= Capacity && oldest.IsValid() {
		delete(s.entries, oldest)
	}
}

// Lookup returns the unexpired domain recorded for addr.
func (s *Store) Lookup(addr netip.Addr) (string, bool) {
	if s == nil {
		return "", false
	}
	addr = addr.Unmap()
	s.mu.Lock()
	defer s.mu.Unlock()
	e, ok := s.entries[addr]
	if !ok {
		return "", false
	}
	if !s.now().Before(e.expires) {
		delete(s.entries, addr)
		return "", false
	}
	return e.domain, true
}

type storeKey struct{}

// WithStore makes the DNS servers and domaindest of the kernels built from
// ctx share s.
func WithStore(ctx context.Context, s *Store) context.Context {
	return context.WithValue(ctx, storeKey{}, s)
}

// FromContext returns the store set by WithStore, or nil.
func FromContext(ctx context.Context) *Store {
	s, _ := ctx.Value(storeKey{}).(*Store)
	return s
}
