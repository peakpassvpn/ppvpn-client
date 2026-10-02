package localdns

import (
	"context"
	"errors"
	"fmt"
	"net/netip"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/sagernet/sing/common/control"
)

const (
	// RetryInterval is the shortest time between two reads of the default
	// interface's resolvers on the same interface: a query without usable
	// servers fails at once instead of reading again.
	RetryInterval = time.Second
	// SoftRefresh is the age after which the next query reads again even
	// without an interface change (a DHCP renewal that changed the DNS).
	SoftRefresh = time.Minute
)

// ErrNoInterface is returned while there is no default interface (no
// network).
var ErrNoInterface = errors.New("local dns: no default interface")

// discoverFunc reads the resolvers of the default interface. source names
// where they came from (scutil-global, scutil-scoped, adapter, ...), and is
// set even when err is not nil.
type discoverFunc func(ctx context.Context, iface control.Interface) (servers []netip.AddrPort, source string, err error)

// siteLocal holds the deprecated fec0::/10 resolvers Windows lists on
// adapters that have no IPv6 DNS of their own (sing-box leaves them out too).
var siteLocal = netip.MustParsePrefix("fec0::/10")

// usable filters the resolvers read for iface: the loopback, unspecified
// and multicast addresses, fec0::/10 and the excluded prefixes (the core's
// own tunnel ranges, current and legacy) are left out, as are duplicates.
// A link-local IPv6 resolver is kept only on iface: without a zone it gets
// iface's index as its zone, with another interface's zone it is dropped
// (the socket is bound to iface, it could not reach it).
func usable(servers []netip.AddrPort, iface control.Interface, exclude []netip.Prefix) []netip.AddrPort {
	var result []netip.AddrPort
	seen := map[netip.AddrPort]bool{}
	for _, server := range servers {
		addr := server.Addr().Unmap()
		port := server.Port()
		if port == 0 {
			port = 53
		}
		bare := addr.WithZone("")
		if !addr.IsValid() || addr.IsUnspecified() || addr.IsLoopback() || addr.IsMulticast() || siteLocal.Contains(bare) || excluded(bare, exclude) {
			continue
		}
		if addr.Is6() && addr.IsLinkLocalUnicast() {
			switch zone := addr.Zone(); zone {
			case "":
				addr = addr.WithZone(strconv.Itoa(iface.Index))
			case iface.Name, strconv.Itoa(iface.Index):
			default:
				continue
			}
		} else {
			addr = bare
		}
		server = netip.AddrPortFrom(addr, port)
		if !seen[server] {
			seen[server] = true
			result = append(result, server)
		}
	}
	return result
}

func excluded(addr netip.Addr, exclude []netip.Prefix) bool {
	for _, prefix := range exclude {
		if prefix.Contains(addr) {
			return true
		}
	}
	return false
}

// cache holds the resolvers of the default interface. It is read again when
// the interface differs from the one they were read on, after invalidate (an
// interface change), after failed (every server failed), and after
// SoftRefresh; never more often than once per RetryInterval on the same
// interface, unless invalidated.
type cache struct {
	discover discoverFunc
	// current returns the default interface, nil when there is none.
	current func() *control.Interface
	exclude []netip.Prefix
	now     func() time.Time
	// changed is called after a read whose outcome differs from the
	// previous one (other servers, interface or source, or no servers).
	changed func(iface control.Interface, source string, servers []netip.AddrPort, err error)

	// generation counts invalidations; a read records the one it ran in.
	generation atomic.Uint64

	mu      sync.Mutex
	read    bool
	readGen uint64
	ifIndex int
	stale   bool
	servers []netip.AddrPort
	err     error
	readAt  time.Time
	triedAt time.Time
	logged  string
}

// invalidate makes the next query read again (an interface change). It does
// not wait for a read in progress.
func (c *cache) invalidate() { c.generation.Add(1) }

// failed marks the servers as suspect after every one of them failed: the
// next query reads again, at most once per RetryInterval.
func (c *cache) failed() {
	c.mu.Lock()
	c.stale = true
	c.mu.Unlock()
}

// get returns the resolvers to ask, reading them first when needed.
// Concurrent callers wait for one read.
func (c *cache) get(ctx context.Context) ([]netip.AddrPort, error) {
	iface := c.current()
	if iface == nil {
		return nil, ErrNoInterface
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	now := c.now()
	gen := c.generation.Load()
	same := c.read && c.readGen == gen && c.ifIndex == iface.Index
	if same && len(c.servers) > 0 && !c.stale && now.Sub(c.readAt) < SoftRefresh {
		return c.servers, nil
	}
	if same && now.Sub(c.triedAt) < RetryInterval {
		if len(c.servers) > 0 {
			return c.servers, nil
		}
		return nil, c.err
	}
	c.triedAt = now
	servers, source, err := c.discover(ctx, *iface)
	if err != nil && same && len(c.servers) > 0 {
		// A refresh that could not read (scutil timed out, ...) keeps the
		// servers read before on the same interface.
		return c.servers, nil
	}
	servers = usable(servers, *iface, c.exclude)
	if err == nil && len(servers) == 0 {
		err = fmt.Errorf("local dns: no DNS servers on %s (%s)", iface.Name, source)
	} else if err != nil {
		err = fmt.Errorf("local dns: read the DNS servers of %s (%s): %w", iface.Name, source, err)
	}
	c.read, c.readGen, c.ifIndex, c.stale = true, gen, iface.Index, false
	c.servers, c.err, c.readAt = servers, err, now
	if key := logKey(*iface, source, servers, err); key != c.logged {
		c.logged = key
		if c.changed != nil {
			c.changed(*iface, source, servers, err)
		}
	}
	if err != nil {
		return nil, err
	}
	return servers, nil
}

func logKey(iface control.Interface, source string, servers []netip.AddrPort, err error) string {
	return fmt.Sprintf("%d %s %s %s %t", iface.Index, iface.Name, source, joinServers(servers), err != nil)
}

func joinServers(servers []netip.AddrPort) string {
	parts := make([]string, len(servers))
	for i, server := range servers {
		parts[i] = server.String()
	}
	return strings.Join(parts, ",")
}
