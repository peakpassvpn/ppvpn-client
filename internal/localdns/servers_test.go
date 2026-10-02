package localdns

import (
	"context"
	"errors"
	"net/netip"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/sagernet/sing/common/control"
)

var tunnel = []netip.Prefix{
	netip.MustParsePrefix("10.60.159.88/30"), netip.MustParsePrefix("fde2:ec40:9312:c7fd::/126"),
	netip.MustParsePrefix("172.19.0.0/30"), netip.MustParsePrefix("fdfe:dcba:9876::/126"),
}

var en0 = control.Interface{Index: 6, Name: "en0"}

func ports(values ...string) []netip.AddrPort {
	var servers []netip.AddrPort
	for _, value := range values {
		server, err := netip.ParseAddrPort(value)
		if err != nil {
			server = netip.AddrPortFrom(netip.MustParseAddr(value), 53)
		}
		servers = append(servers, server)
	}
	return servers
}

func TestUsableLeavesOutTunnelLoopbackAndForeignLinkLocal(t *testing.T) {
	got := usable(ports(
		"10.10.0.3", "172.19.0.2", "10.60.159.90", "fde2:ec40:9312:c7fd::2", "fdfe:dcba:9876::2",
		"127.0.0.1", "::1", "0.0.0.0", "fec0:0:0:ffff::1", "224.0.0.251",
		"fe80::1%en0", "fe80::2%en1", "fe80::3", "fe80::4%6",
		"::ffff:192.168.1.1", "10.10.0.3", "[2001:db8::53]:5353",
	), en0, tunnel)
	want := "10.10.0.3:53,[fe80::1%en0]:53,[fe80::3%6]:53,[fe80::4%6]:53,192.168.1.1:53,[2001:db8::53]:5353"
	if joinServers(got) != want {
		t.Fatalf("got  %s\nwant %s", joinServers(got), want)
	}
}

// The sample of the desktop's macdns.rs test: tunnel addresses old and new
// mixed into the primary service's DNS, and a link-local one with a zone.
const scutilGlobal = "<dictionary> {\n  ARPResolvedHardwareAddress : 0:11:22:33:44:55\n  PrimaryInterface : en0\n  PrimaryService : 4A8B1E2C-0000-0000-0000-000000000000\n  Router : 10.10.0.1\n}\n" +
	"  No such key\n" +
	"<dictionary> {\n  ServerAddresses : <array> {\n    0 : 10.10.0.3\n    1 : 172.19.0.2\n    2 : fe80::1%en0\n    3 : fdfe:dcba:9876::2\n    4 : 2001:db8::53\n    5 : 10.60.159.90\n    6 : fde2:ec40:9312:c7fd::2\n  }\n  SearchDomains : <array> {\n    0 : lan\n  }\n}\n"

func TestGlobalServers(t *testing.T) {
	servers, ok := globalServers(scutilGlobal, "en0", 6)
	if !ok {
		t.Fatal("en0 is primary")
	}
	if got := joinServers(usable(servers, en0, tunnel)); got != "10.10.0.3:53,[fe80::1%en0]:53,[2001:db8::53]:53" {
		t.Fatalf("servers %s", got)
	}
	// Another primary interface (configd behind a switch): not ours.
	if _, ok := globalServers(scutilGlobal, "en1", 7); ok {
		t.Fatal("en1 is not primary")
	}
	// IPv6-only network: the primary interface comes from Global/IPv6.
	v6only := "  No such key\n<dictionary> {\n  PrimaryInterface : en1\n}\n<dictionary> {\n  ServerAddresses : <array> {\n    0 : 2001:db8::53\n  }\n}\n"
	if servers, ok := globalServers(v6only, "en1", 7); !ok || joinServers(servers) != "[2001:db8::53]:53" {
		t.Fatalf("v6-only: %v %v", servers, ok)
	}
	// Another VPN's service is primary (seen on a Mac running one): its utun
	// is the primary interface and the global DNS is its own, recorded
	// with its interface index. Not en0's, whatever the primary says.
	otherVPN := "<dictionary> {\n  PrimaryInterface : utun4\n  Router : 10.8.0.1\n}\n  No such key\n" +
		"<dictionary> {\n  ServerAddresses : <array> {\n    0 : 10.8.0.53\n  }\n  __CONFIGURATION_ID__ : Default: 0\n  __FLAGS__ : 2\n  __IF_INDEX__ : 29\n  __ORDER__ : 0\n}\n"
	if _, ok := globalServers(otherVPN, "en0", 6); ok {
		t.Fatal("another VPN's DNS taken for en0")
	}
	if servers, ok := globalServers(otherVPN, "utun4", 29); !ok || joinServers(servers) != "10.8.0.53:53" {
		t.Fatalf("by __IF_INDEX__: %v %v", servers, ok)
	}
	// __IF_INDEX__ wins over a primary interface configd has not updated.
	behind := strings.Replace(otherVPN, "utun4", "en0", 1)
	if _, ok := globalServers(behind, "en0", 6); ok {
		t.Fatal("__IF_INDEX__ 29 is not en0")
	}
	// No DNS at all yet (DHCP pending).
	pending := "<dictionary> {\n  PrimaryInterface : en0\n}\n  No such key\n  No such key\n"
	if servers, ok := globalServers(pending, "en0", 6); ok || len(servers) != 0 {
		t.Fatalf("pending: %v %v", servers, ok)
	}
}

const scutilDNS = `DNS configuration

resolver #1
  nameserver[0] : 10.60.159.90
  nameserver[1] : fde2:ec40:9312:c7fd::2
  flags    : Supplemental, Request A records, Request AAAA records
  reach    : 0x00000003 (Reachable,Transient Connection)
  order    : 100000

resolver #2
  search domain[0] : lan
  nameserver[0] : 10.10.0.3
  if_index : 6 (en0)
  flags    : Request A records
  reach    : 0x00020002 (Reachable,Directly Reachable Address)
  order    : 200000

DNS configuration (for scoped queries)

resolver #1
  domain   : corp.example
  nameserver[0] : 10.99.0.53
  if_index : 6 (en0)
  flags    : Scoped, Request A records
  reach    : 0x00000002 (Reachable)

resolver #2
  search domain[0] : lan
  nameserver[0] : 10.10.0.3
  nameserver[1] : fe80::1%en0
  if_index : 6 (en0)
  flags    : Scoped, Request A records
  reach    : 0x00020002 (Reachable,Directly Reachable Address)

resolver #3
  nameserver[0] : 192.168.8.1
  if_index : 16 (en7)
  flags    : Scoped, Request A records
  reach    : 0x00020002 (Reachable,Directly Reachable Address)
`

func TestScopedServers(t *testing.T) {
	// The domain-specific (split DNS) resolver on en0 is skipped.
	if got := joinServers(scopedServers(scutilDNS, 6)); got != "10.10.0.3:53,[fe80::1%en0]:53" {
		t.Fatalf("en0: %s", got)
	}
	if got := joinServers(scopedServers(scutilDNS, 16)); got != "192.168.8.1:53" {
		t.Fatalf("en7: %s", got)
	}
	if got := scopedServers(scutilDNS, 99); got != nil {
		t.Fatalf("unknown index: %v", got)
	}
	if got := scopedServers("DNS configuration\n\nresolver #1\n  nameserver[0] : 1.1.1.1\n", 6); got != nil {
		t.Fatalf("no scoped section: %v", got)
	}
}

// fakeSource is a discover function returning what the test sets for each
// interface, counting reads.
type fakeSource struct {
	mu      sync.Mutex
	servers map[string][]netip.AddrPort
	err     error
	reads   atomic.Int32
	block   chan struct{}
}

func (f *fakeSource) set(name string, servers ...string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.servers[name] = ports(servers...)
}

func (f *fakeSource) discover(_ context.Context, iface control.Interface) ([]netip.AddrPort, string, error) {
	f.reads.Add(1)
	if f.block != nil {
		<-f.block
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.servers[iface.Name], "fake", f.err
}

type logged struct {
	iface   string
	servers string
	err     bool
}

func newTestCache(source *fakeSource, iface **control.Interface, now *time.Time) (*cache, *[]logged) {
	var lines []logged
	return &cache{
		discover: source.discover,
		current:  func() *control.Interface { return *iface },
		exclude:  tunnel,
		now:      func() time.Time { return *now },
		changed: func(iface control.Interface, _ string, servers []netip.AddrPort, err error) {
			lines = append(lines, logged{iface.Name, joinServers(servers), err != nil})
		},
	}, &lines
}

func TestCacheFollowsInterfaceChanges(t *testing.T) {
	source := &fakeSource{servers: map[string][]netip.AddrPort{}}
	source.set("en0", "192.168.1.1")
	current := &control.Interface{Index: 6, Name: "en0"}
	now := time.Unix(1000, 0)
	c, lines := newTestCache(source, &current, &now)
	get := func() string {
		t.Helper()
		servers, err := c.get(context.Background())
		if err != nil {
			return "error: " + err.Error()
		}
		return joinServers(servers)
	}

	if got := get(); got != "192.168.1.1:53" || source.reads.Load() != 1 {
		t.Fatalf("first: %s, %d reads", got, source.reads.Load())
	}
	now = now.Add(30 * time.Second)
	if got := get(); got != "192.168.1.1:53" || source.reads.Load() != 1 {
		t.Fatalf("cached: %s, %d reads", got, source.reads.Load())
	}

	// Switch to another Wi-Fi on the same interface: the monitor fires (new
	// address), the next query reads at once, well within RetryInterval.
	source.set("en0", "10.0.0.1")
	c.invalidate()
	now = now.Add(time.Millisecond)
	if got := get(); got != "10.0.0.1:53" || source.reads.Load() != 2 {
		t.Fatalf("after the switch: %s, %d reads", got, source.reads.Load())
	}

	// A new default interface is read even without the callback.
	source.set("en7", "192.168.8.1")
	current = &control.Interface{Index: 16, Name: "en7"}
	if got := get(); got != "192.168.8.1:53" || source.reads.Load() != 3 {
		t.Fatalf("new interface: %s, %d reads", got, source.reads.Load())
	}

	// No network: fails at once, without a read.
	current = nil
	if _, err := c.get(context.Background()); !errors.Is(err, ErrNoInterface) || source.reads.Load() != 3 {
		t.Fatalf("no interface: %v, %d reads", err, source.reads.Load())
	}

	want := []logged{{"en0", "192.168.1.1:53", false}, {"en0", "10.0.0.1:53", false}, {"en7", "192.168.8.1:53", false}}
	if len(*lines) != len(want) {
		t.Fatalf("logged %v", *lines)
	}
	for i := range want {
		if (*lines)[i] != want[i] {
			t.Fatalf("logged %v", *lines)
		}
	}
}

// DHCP has not handed out DNS yet: queries fail at once with a clear error
// (never 127.0.0.1, never a 5 s timeout), and the interface is read again at
// most once per RetryInterval until servers appear.
func TestCacheFailsFastWithoutServers(t *testing.T) {
	source := &fakeSource{servers: map[string][]netip.AddrPort{}}
	// Only addresses that must never be used.
	source.set("en0", "127.0.0.1", "::1", "10.60.159.90", "172.19.0.2", "fec0:0:0:ffff::1")
	current := &control.Interface{Index: 6, Name: "en0"}
	now := time.Unix(1000, 0)
	c, lines := newTestCache(source, &current, &now)

	for i := 0; i < 5; i++ {
		started := time.Now()
		_, err := c.get(context.Background())
		if err == nil || !strings.Contains(err.Error(), "no DNS servers on en0") {
			t.Fatalf("query %d: %v", i, err)
		}
		if time.Since(started) > 100*time.Millisecond {
			t.Fatalf("query %d took %v", i, time.Since(started))
		}
		now = now.Add(100 * time.Millisecond)
	}
	if source.reads.Load() != 1 {
		t.Fatalf("%d reads within RetryInterval", source.reads.Load())
	}
	now = now.Add(RetryInterval)
	source.set("en0", "192.168.1.1")
	if servers, err := c.get(context.Background()); err != nil || joinServers(servers) != "192.168.1.1:53" || source.reads.Load() != 2 {
		t.Fatalf("after DHCP: %v %v, %d reads", servers, err, source.reads.Load())
	}
	if len(*lines) != 2 || !(*lines)[0].err || (*lines)[1].err {
		t.Fatalf("logged %v", *lines)
	}

	// An invalidation reads at once even right after a read.
	source.set("en0")
	c.invalidate()
	if _, err := c.get(context.Background()); err == nil || source.reads.Load() != 3 {
		t.Fatalf("invalidated: %v, %d reads", err, source.reads.Load())
	}
}

func TestCacheRefreshes(t *testing.T) {
	source := &fakeSource{servers: map[string][]netip.AddrPort{}}
	source.set("en0", "192.168.1.1")
	current := &control.Interface{Index: 6, Name: "en0"}
	now := time.Unix(1000, 0)
	c, lines := newTestCache(source, &current, &now)
	if _, err := c.get(context.Background()); err != nil {
		t.Fatal(err)
	}

	// Every server failed: read again on the next query, at most once per
	// RetryInterval.
	source.set("en0", "192.168.1.2")
	c.failed()
	if servers, _ := c.get(context.Background()); source.reads.Load() != 1 || joinServers(servers) != "192.168.1.1:53" {
		t.Fatalf("within RetryInterval: %v, %d reads", servers, source.reads.Load())
	}
	now = now.Add(RetryInterval)
	if servers, _ := c.get(context.Background()); source.reads.Load() != 2 || joinServers(servers) != "192.168.1.2:53" {
		t.Fatalf("after failure: %v, %d reads", servers, source.reads.Load())
	}

	// Soft refresh after SoftRefresh; a read that fails keeps the servers.
	now = now.Add(SoftRefresh)
	source.err = errors.New("scutil timed out")
	if servers, err := c.get(context.Background()); err != nil || joinServers(servers) != "192.168.1.2:53" || source.reads.Load() != 3 {
		t.Fatalf("failed refresh: %v %v, %d reads", servers, err, source.reads.Load())
	}
	// A read error with nothing to keep is reported.
	current = &control.Interface{Index: 7, Name: "en1"}
	if _, err := c.get(context.Background()); err == nil || !strings.Contains(err.Error(), "scutil timed out") {
		t.Fatalf("read error: %v", err)
	}
	if len(*lines) != 3 || !(*lines)[2].err {
		t.Fatalf("logged %v", *lines)
	}
}

// Concurrent queries after an invalidation share one read.
func TestCacheReadsOnceForConcurrentQueries(t *testing.T) {
	source := &fakeSource{servers: map[string][]netip.AddrPort{}, block: make(chan struct{})}
	source.set("en0", "192.168.1.1")
	current := &control.Interface{Index: 6, Name: "en0"}
	now := time.Unix(1000, 0)
	c, _ := newTestCache(source, &current, &now)
	var wg sync.WaitGroup
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if servers, err := c.get(context.Background()); err != nil || len(servers) != 1 {
				t.Errorf("%v %v", servers, err)
			}
		}()
	}
	for source.reads.Load() == 0 {
		time.Sleep(time.Millisecond)
	}
	close(source.block)
	wg.Wait()
	if source.reads.Load() != 1 {
		t.Fatalf("%d reads", source.reads.Load())
	}
}
