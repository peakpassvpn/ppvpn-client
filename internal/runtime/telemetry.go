package runtime

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"net"
	"sync"
	"sync/atomic"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/domaindest"
	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing/common/buf"
	"github.com/sagernet/sing/common/bufio"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
)

type Traffic struct {
	UploadBytes   uint64    `json:"upload_bytes"`
	DownloadBytes uint64    `json:"download_bytes"`
	MeasuredAt    time.Time `json:"measured_at"`
}
type Connection struct {
	ID            string    `json:"id"`
	OutboundTag   string    `json:"-"`
	NodeID        string    `json:"node_id"`
	Network       string    `json:"network"`
	Destination   string    `json:"destination"`
	UploadBytes   uint64    `json:"upload_bytes"`
	DownloadBytes uint64    `json:"download_bytes"`
	StartedAt     time.Time `json:"started_at"`
}
type tracked struct {
	connection       Connection
	upload, download atomic.Uint64
	// lastActive is when a byte last went either way (unix nanoseconds),
	// for closing idle connections of a draining kernel.
	lastActive atomic.Int64
	// gen is the kernel generation that routed the connection; metadata is
	// what its router matched; closeConn closes it (set once wrapped).
	gen       uint64
	metadata  adapter.InboundContext
	closeConn func() error
	// route is the outbound chain the connection took, outermost first
	// (e.g. domaindest wrapper, selector, node group, ingress), resolved
	// when it was routed; nodeID is the logical node that chain reached,
	// by the build of the kernel that routed it ("" for none, e.g. direct).
	route  []string
	nodeID string
}
type telemetry struct {
	upload, download atomic.Uint64
	mu               sync.RWMutex
	connections      map[string]*tracked
	// log receives one debug line per routed connection; nil logs nothing.
	log atomic.Pointer[corelog.Logger]
}

func newTelemetry() *telemetry { return &telemetry{connections: map[string]*tracked{}} }

// counters returns the read and write counters for the routed conn, which is
// the inbound (client) side: what is read from it is the client's upload
// toward the remote, what is written to it is the remote's download back to
// the client. (bufio.NewCounterConn takes read counters first, as sing-box's
// clash API does with upload; before 0.5.16 the two were swapped.)
func (item *tracked) counters(t *telemetry) (upload, download []N.CountFunc) {
	return []N.CountFunc{func(n int64) {
			item.upload.Add(uint64(n))
			t.upload.Add(uint64(n))
			item.lastActive.Store(time.Now().UnixNano())
		}},
		[]N.CountFunc{func(n int64) {
			item.download.Add(uint64(n))
			t.download.Add(uint64(n))
			item.lastActive.Store(time.Now().UnixNano())
		}}
}
func (t *telemetry) RoutedConnection(ctx context.Context, conn net.Conn, metadata adapter.InboundContext, rule adapter.Rule, outbound adapter.Outbound) net.Conn {
	return t.routedConnection(0, conn, metadata, rule, outbound)
}
func (t *telemetry) RoutedPacketConnection(ctx context.Context, conn N.PacketConn, metadata adapter.InboundContext, rule adapter.Rule, outbound adapter.Outbound) N.PacketConn {
	return t.routedPacketConnection(0, conn, metadata, rule, outbound)
}
func (t *telemetry) routedConnection(gen uint64, conn net.Conn, metadata adapter.InboundContext, rule adapter.Rule, outbound adapter.Outbound) net.Conn {
	conn = takeCachedConn(conn)
	item := t.add(gen, metadata, outbound)
	t.logRouted(item.connection.ID, metadata, rule, outbound)
	upload, download := item.counters(t)
	counted := bufio.NewCounterConn(conn, upload, download)
	wrapped := &trackedConn{ExtendedConn: counted, id: item.connection.ID, onClose: func() { t.remove(item.connection.ID) }}
	t.setCloser(item, wrapped.Close)
	return wrapped
}
func (t *telemetry) routedPacketConnection(gen uint64, conn N.PacketConn, metadata adapter.InboundContext, rule adapter.Rule, outbound adapter.Outbound) N.PacketConn {
	conn = takeCachedPacketConn(conn)
	item := t.add(gen, metadata, outbound)
	t.logRouted(item.connection.ID, metadata, rule, outbound)
	upload, download := item.counters(t)
	counted := bufio.NewCounterPacketConn(conn, upload, download)
	wrapped := &trackedPacketConn{PacketConn: counted, id: item.connection.ID, onClose: func() { t.remove(item.connection.ID) }}
	t.setCloser(item, wrapped.Close)
	return wrapped
}
func (t *telemetry) add(gen uint64, metadata adapter.InboundContext, outbound adapter.Outbound) *tracked {
	tag := ""
	if outbound != nil {
		tag = adapter.OutboundTag(outbound)
	}
	item := &tracked{gen: gen, metadata: metadata, connection: Connection{ID: randomConnectionID(), OutboundTag: tag, Network: metadata.Network, Destination: metadata.Destination.String(), StartedAt: time.Now()}}
	item.lastActive.Store(item.connection.StartedAt.UnixNano())
	t.mu.Lock()
	t.connections[item.connection.ID] = item
	t.mu.Unlock()
	return item
}

func (t *telemetry) setCloser(item *tracked, closeConn func() error) {
	t.mu.Lock()
	item.closeConn = closeConn
	t.mu.Unlock()
}

// kernelTracker attributes the connections one kernel routes to its
// generation, so all kernels can share one telemetry, and records the
// outbound chain each one took.
type kernelTracker struct {
	t         *telemetry
	gen       uint64
	outbounds adapter.OutboundManager
	// nodes maps this kernel's node and ingress outbound tags to node IDs
	// (its build's OutboundNodes; see withOutboundNodes).
	nodes map[string]string
}

type outboundNodesKey struct{}

// withOutboundNodes gives the kernel built under ctx its build's map of
// outbound tags to node IDs, so its connections record their node.
func withOutboundNodes(ctx context.Context, nodes map[string]string) context.Context {
	return context.WithValue(ctx, outboundNodesKey{}, nodes)
}

func outboundNodesFrom(ctx context.Context) map[string]string {
	nodes, _ := ctx.Value(outboundNodesKey{}).(map[string]string)
	return nodes
}

func (k kernelTracker) RoutedConnection(_ context.Context, conn net.Conn, metadata adapter.InboundContext, rule adapter.Rule, outbound adapter.Outbound) net.Conn {
	wrapped := k.t.routedConnection(k.gen, conn, metadata, rule, outbound)
	k.setRoute(wrapped, outbound)
	return wrapped
}
func (k kernelTracker) RoutedPacketConnection(_ context.Context, conn N.PacketConn, metadata adapter.InboundContext, rule adapter.Rule, outbound adapter.Outbound) N.PacketConn {
	wrapped := k.t.routedPacketConnection(k.gen, conn, metadata, rule, outbound)
	k.setRoute(wrapped, outbound)
	return wrapped
}

// setRoute records the chain outbound resolves to and the node it reaches,
// the first tag of the chain this kernel's build attributes to a node.
func (k kernelTracker) setRoute(wrapped any, outbound adapter.Outbound) {
	chain := k.route(outbound)
	node := ""
	for _, tag := range chain {
		if id, ok := k.nodes[tag]; ok {
			node = id
			break
		}
	}
	k.t.setRoute(wrapped, chain, node)
}

// route follows groups (anything with Now(): selector, domaindest, a node's
// failover group) to the outbound that carries the connection.
func (k kernelTracker) route(outbound adapter.Outbound) []string {
	var chain []string
	for depth := 0; outbound != nil && depth < 8; depth++ {
		chain = append(chain, adapter.OutboundTag(outbound))
		group, ok := outbound.(interface{ Now() string })
		if !ok || k.outbounds == nil {
			break
		}
		outbound, _ = k.outbounds.Outbound(group.Now())
	}
	return chain
}

// setRoute stores the chain on the item behind a wrapped connection.
func (t *telemetry) setRoute(wrapped any, chain []string, nodeID string) {
	var id string
	switch conn := wrapped.(type) {
	case *trackedConn:
		id = conn.id
	case *trackedPacketConn:
		id = conn.id
	}
	t.mu.Lock()
	if item, ok := t.connections[id]; ok {
		item.route = chain
		item.nodeID = nodeID
	}
	t.mu.Unlock()
}

// trackedView is a consistent copy of what a switch needs to judge one
// tracked connection (route is written after the item is added).
type trackedView struct {
	item        *tracked
	outboundTag string
	metadata    adapter.InboundContext
	route       []string
	nodeID      string
	lastActive  time.Time
	bytes       uint64 // both directions so far
}

// generation lists the open connections of kernel gen.
func (t *telemetry) generation(gen uint64) []trackedView {
	return t.generations(map[uint64]bool{gen: true})
}

// generations lists the open connections of the kernels in gens, in one
// pass over the table.
func (t *telemetry) generations(gens map[uint64]bool) []trackedView {
	t.mu.RLock()
	defer t.mu.RUnlock()
	var items []trackedView
	for _, item := range t.connections {
		if gens[item.gen] {
			items = append(items, trackedView{item: item, outboundTag: item.connection.OutboundTag, metadata: item.metadata, route: append([]string(nil), item.route...), nodeID: item.nodeID, lastActive: time.Unix(0, item.lastActive.Load()), bytes: item.upload.Load() + item.download.Load()})
		}
	}
	return items
}

// closeConnection closes one tracked connection (from another goroutine than
// its copy loop: the copy ends with an error and sing-box closes both sides).
func (t *telemetry) closeConnection(item *tracked) {
	t.mu.RLock()
	closeConn := item.closeConn
	t.mu.RUnlock()
	if closeConn != nil {
		_ = closeConn()
	}
}

// logRouted writes the debug line of a routed connection: where it came from,
// the domain route rules matched against (route_domain: sniffed, or from the
// DNS reverse mapping; the HTTP sniffer may leave an address there), the
// rule, the outbound, and what the node is asked to connect to (target_kind
// "domain" when domaindest hands over a name).
func (t *telemetry) logRouted(id string, metadata adapter.InboundContext, rule adapter.Rule, outbound adapter.Outbound) {
	log := t.log.Load()
	if !log.DebugEnabled() {
		return
	}
	tag, target := "", metadata
	if outbound != nil {
		tag = adapter.OutboundTag(outbound)
		if wrapper, ok := outbound.(*domaindest.Outbound); ok {
			wrapper.Restore(&target)
			tag = wrapper.Now()
		}
	}
	kind := "ip"
	if target.Destination.IsFqdn() {
		kind = "domain"
	}
	ruleName := "final"
	if rule != nil {
		ruleName = rule.String()
	}
	log.Debug("connection", "id", id, "inbound", metadata.Inbound, "network", metadata.Network,
		"destination", metadata.Destination.String(), "route_domain", metadata.Domain, "protocol", metadata.Protocol,
		"rule", ruleName, "outbound", tag, "target", target.Destination.String(), "target_kind", kind)
}

func (t *telemetry) remove(id string) { t.mu.Lock(); delete(t.connections, id); t.mu.Unlock() }
func (t *telemetry) snapshot() (Traffic, []Connection) {
	traffic := Traffic{UploadBytes: t.upload.Load(), DownloadBytes: t.download.Load(), MeasuredAt: time.Now()}
	t.mu.RLock()
	connections := make([]Connection, 0, len(t.connections))
	for _, item := range t.connections {
		value := item.connection
		value.UploadBytes = item.upload.Load()
		value.DownloadBytes = item.download.Load()
		connections = append(connections, value)
	}
	t.mu.RUnlock()
	return traffic, connections
}
func randomConnectionID() string {
	value := make([]byte, 16)
	if _, err := rand.Read(value); err != nil {
		return "unavailable"
	}
	return hex.EncodeToString(value)
}

type trackedConn struct {
	N.ExtendedConn
	id      string
	once    sync.Once
	onClose func()
}

func (c *trackedConn) Close() error { c.once.Do(c.onClose); return c.ExtendedConn.Close() }

type trackedPacketConn struct {
	N.PacketConn
	id      string
	once    sync.Once
	onClose func()
}

func (c *trackedPacketConn) Close() error { c.once.Do(c.onClose); return c.PacketConn.Close() }

// A sniffed connection arrives as bufio.CachedConn / CachedPacketConn holding
// the sniffed bytes. Once wrapped by the counters, sing-box's copy loop can no
// longer take that cache atomically (ReadCached) and reads it through
// Read/ReadPacket, which clear the buffer unsynchronized while Close from the
// other copy direction reads it: a data race that can release the pooled
// buffer twice. So the tracker takes the cache itself (atomically, marking it
// taken so Close leaves it alone), copies the bytes into a prefix only the
// reader touches, and releases the buffer at once.

func takeCachedConn(conn net.Conn) net.Conn {
	var prefix []byte
	for {
		cached, ok := conn.(*bufio.CachedConn)
		if !ok {
			break
		}
		// The outermost cache is read first.
		if buffer := cached.ReadCached(); buffer != nil {
			prefix = append(prefix, buffer.Bytes()...)
			buffer.Release()
		}
		conn = cached.Conn
	}
	if len(prefix) == 0 {
		return conn
	}
	return &prefixConn{Conn: conn, prefix: prefix}
}

// prefixConn serves prefix before reading conn. Only the reading goroutine
// touches prefix.
type prefixConn struct {
	net.Conn
	prefix []byte
}

func (c *prefixConn) Read(p []byte) (int, error) {
	if len(c.prefix) > 0 {
		n := copy(p, c.prefix)
		c.prefix = c.prefix[n:]
		return n, nil
	}
	return c.Conn.Read(p)
}

func (c *prefixConn) Upstream() any { return c.Conn }

func takeCachedPacketConn(conn N.PacketConn) N.PacketConn {
	var packets []cachedPacket
	for {
		cached, ok := conn.(*bufio.CachedPacketConn)
		if !ok {
			break
		}
		if packet := cached.ReadCachedPacket(); packet != nil {
			if packet.Buffer != nil {
				packets = append(packets, cachedPacket{data: append([]byte(nil), packet.Buffer.Bytes()...), destination: packet.Destination})
				packet.Buffer.Release()
			}
			N.PutPacketBuffer(packet)
		}
		conn = cached.PacketConn
	}
	if len(packets) == 0 {
		return conn
	}
	return &prefixPacketConn{PacketConn: conn, packets: packets}
}

type cachedPacket struct {
	data        []byte
	destination M.Socksaddr
}

// prefixPacketConn returns the cached packets before reading conn. Only the
// reading goroutine touches packets.
type prefixPacketConn struct {
	N.PacketConn
	packets []cachedPacket
}

func (c *prefixPacketConn) ReadPacket(buffer *buf.Buffer) (M.Socksaddr, error) {
	if len(c.packets) > 0 {
		packet := c.packets[0]
		c.packets = c.packets[1:]
		if _, err := buffer.Write(packet.data); err != nil {
			return M.Socksaddr{}, err
		}
		return packet.destination, nil
	}
	return c.PacketConn.ReadPacket(buffer)
}

func (c *prefixPacketConn) Upstream() any { return c.PacketConn }
