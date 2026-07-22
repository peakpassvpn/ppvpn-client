package runtime

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"net"
	"sync"
	"sync/atomic"
	"time"

	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing/common/bufio"
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
}
type telemetry struct {
	upload, download atomic.Uint64
	mu               sync.RWMutex
	connections      map[string]*tracked
}

func newTelemetry() *telemetry { return &telemetry{connections: map[string]*tracked{}} }
func (t *telemetry) RoutedConnection(_ context.Context, conn net.Conn, metadata adapter.InboundContext, _ adapter.Rule, outbound adapter.Outbound) net.Conn {
	item := t.add(metadata, outbound)
	counted := bufio.NewCounterConn(conn, []N.CountFunc{func(n int64) { item.download.Add(uint64(n)); t.download.Add(uint64(n)) }}, []N.CountFunc{func(n int64) { item.upload.Add(uint64(n)); t.upload.Add(uint64(n)) }})
	return &trackedConn{ExtendedConn: counted, onClose: func() { t.remove(item.connection.ID) }}
}
func (t *telemetry) RoutedPacketConnection(_ context.Context, conn N.PacketConn, metadata adapter.InboundContext, _ adapter.Rule, outbound adapter.Outbound) N.PacketConn {
	item := t.add(metadata, outbound)
	counted := bufio.NewCounterPacketConn(conn, []N.CountFunc{func(n int64) { item.download.Add(uint64(n)); t.download.Add(uint64(n)) }}, []N.CountFunc{func(n int64) { item.upload.Add(uint64(n)); t.upload.Add(uint64(n)) }})
	return &trackedPacketConn{PacketConn: counted, onClose: func() { t.remove(item.connection.ID) }}
}
func (t *telemetry) add(metadata adapter.InboundContext, outbound adapter.Outbound) *tracked {
	tag := ""
	if outbound != nil {
		tag = adapter.OutboundTag(outbound)
	}
	item := &tracked{connection: Connection{ID: randomConnectionID(), OutboundTag: tag, Network: metadata.Network, Destination: metadata.Destination.String(), StartedAt: time.Now()}}
	t.mu.Lock()
	t.connections[item.connection.ID] = item
	t.mu.Unlock()
	return item
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
	once    sync.Once
	onClose func()
}

func (c *trackedConn) Close() error { c.once.Do(c.onClose); return c.ExtendedConn.Close() }

type trackedPacketConn struct {
	N.PacketConn
	once    sync.Once
	onClose func()
}

func (c *trackedPacketConn) Close() error { c.once.Do(c.onClose); return c.PacketConn.Close() }
