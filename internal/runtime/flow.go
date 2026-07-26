package runtime

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/netip"
	"strings"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/routing"
)

const (
	maxFlowChunk       = 1024 * 1024
	maxDatagramPayload = 65535
	maxDatagramWrite   = 65507
)

// FlowConnection is a proxied stream or connected datagram transport owned by
// the platform adapter. It never exposes core implementation types.
type FlowConnection struct {
	mu      sync.Mutex
	readMu  sync.Mutex
	writeMu sync.Mutex
	conn    net.Conn
	network string
	nodeID  string
	closed  bool
}

func (c *Core) ClassifyFlow(flow routing.Flow) (routing.Decision, error) {
	if err := validateFlow(flow); err != nil {
		return routing.Decision{}, err
	}
	c.mu.RLock()
	classifier, selected, generation := c.classifier, c.selected, c.routingGeneration
	running := c.engine != nil
	key, keyOK := c.flowAuthorizationKey, c.flowAuthorizationOK
	c.mu.RUnlock()
	if classifier == nil {
		return routing.Decision{}, ErrProfileNotApplied
	}
	if !running {
		return routing.Decision{}, fmt.Errorf("core is not running")
	}
	if !keyOK {
		return routing.Decision{}, fmt.Errorf("flow authorization is unavailable")
	}
	decision := classifier.Classify(flow, selected)
	decision.Snapshot = generation
	decision.Authorization = authorizeDecision(key, flow, decision)
	return decision, nil
}

func (c *Core) OpenFlow(ctx context.Context, flow routing.Flow, decision routing.Decision) (*FlowConnection, error) {
	if err := validateFlow(flow); err != nil {
		return nil, err
	}
	c.operation.RLock()
	defer c.operation.RUnlock()
	c.mu.RLock()
	instance, built, generation := c.engine, c.built, c.routingGeneration
	key, keyOK := c.flowAuthorizationKey, c.flowAuthorizationOK
	c.mu.RUnlock()
	if built == nil {
		return nil, ErrProfileNotApplied
	}
	dialer, ok := instance.(flowEngine)
	if !ok {
		return nil, fmt.Errorf("core flow adapter is unavailable")
	}
	if !keyOK ||
		decision.Snapshot == 0 ||
		decision.Snapshot != generation ||
		!hmac.Equal(
			[]byte(decision.Authorization),
			[]byte(authorizeDecision(key, flow, decision)),
		) {
		return nil, fmt.Errorf("flow decision is stale or invalid")
	}
	if decision.Type != "proxy" || decision.NodeID == "" {
		return nil, fmt.Errorf("flow decision is not proxy")
	}
	outboundTag, ok := built.NodeTags[decision.NodeID]
	if !ok {
		return nil, fmt.Errorf("flow proxy node is unavailable")
	}
	network := strings.ToLower(flow.Protocol)
	if network != "tcp" && network != "udp" {
		return nil, fmt.Errorf("flow protocol must be tcp or udp")
	}
	if flow.DestinationPort == 0 {
		return nil, fmt.Errorf("flow destination port is required")
	}
	host := ""
	if normalized, valid := profileHost(flow.Hostname); valid {
		host = normalized
	} else if address, err := netip.ParseAddr(flow.DestinationIP); err == nil {
		host = address.String()
	}
	if host == "" {
		return nil, fmt.Errorf("flow destination is required")
	}
	conn, err := dialer.dialFlow(ctx, network, outboundTag, host, flow.DestinationPort)
	if err != nil {
		return nil, fmt.Errorf("open proxied flow: %w", err)
	}
	return &FlowConnection{conn: conn, network: network, nodeID: decision.NodeID}, nil
}

func newFlowAuthorizationKey() ([32]byte, bool) {
	var key [32]byte
	_, err := rand.Read(key[:])
	return key, err == nil
}

func authorizeDecision(key [32]byte, flow routing.Flow, decision routing.Decision) string {
	decision.Authorization = ""
	message, _ := json.Marshal(struct {
		Flow     routing.Flow     `json:"flow"`
		Decision routing.Decision `json:"decision"`
	}{Flow: flow, Decision: decision})
	mac := hmac.New(sha256.New, key[:])
	_, _ = mac.Write(message)
	return fmt.Sprintf("%x", mac.Sum(nil))
}

func profileHost(value string) (string, bool) {
	// Kept local to avoid accepting IP literals through the hostname path.
	return profile.NormalizeDomain(value)
}

func validateFlow(flow routing.Flow) error {
	switch flow.Entry {
	case routing.EntryTransparent, routing.EntryPlatformSafety:
	case routing.EntryLocalProxy:
		if flow.LocalProxyNodeID == "" {
			return fmt.Errorf("local proxy node is required")
		}
	default:
		return fmt.Errorf("flow entry is unsupported")
	}
	if flow.Protocol != "tcp" && flow.Protocol != "udp" {
		return fmt.Errorf("flow protocol must be tcp or udp")
	}
	if flow.DestinationPort == 0 {
		return fmt.Errorf("flow destination port is required")
	}
	if _, valid := profileHost(flow.Hostname); valid {
		return nil
	}
	if _, err := netip.ParseAddr(flow.DestinationIP); err != nil {
		return fmt.Errorf("flow destination is required")
	}
	return nil
}

func (c *FlowConnection) Network() string { return c.network }
func (c *FlowConnection) NodeID() string  { return c.nodeID }

func (c *FlowConnection) Read(maxBytes int, timeout time.Duration) ([]byte, error) {
	c.readMu.Lock()
	defer c.readMu.Unlock()
	maxRead := maxFlowChunk
	if c.network == "udp" {
		maxRead = maxDatagramPayload
	}
	if maxBytes <= 0 || maxBytes > maxRead {
		return nil, fmt.Errorf("read size must be between 1 and %d", maxRead)
	}
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil, net.ErrClosed
	}
	conn := c.conn
	c.mu.Unlock()
	if timeout > 0 {
		if err := conn.SetReadDeadline(time.Now().Add(timeout)); err != nil {
			return nil, err
		}
		defer conn.SetReadDeadline(time.Time{})
	}
	readSize := maxBytes
	if c.network == "udp" {
		// Read the complete UDP datagram into an internal buffer first. Reading
		// directly into maxBytes would let net.Conn silently truncate a
		// datagram while still reporting success.
		readSize = maxDatagramPayload
	}
	buffer := make([]byte, readSize)
	n, err := conn.Read(buffer)
	if err != nil {
		return nil, err
	}
	if c.network == "udp" && n > maxBytes {
		return nil, fmt.Errorf("datagram payload %d exceeds caller buffer %d", n, maxBytes)
	}
	return buffer[:n], nil
}

func (c *FlowConnection) Write(data []byte, timeout time.Duration) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return net.ErrClosed
	}
	conn, network := c.conn, c.network
	c.mu.Unlock()
	if network == "udp" {
		if len(data) > maxDatagramWrite {
			return fmt.Errorf("datagram payload must not exceed %d", maxDatagramWrite)
		}
	} else if len(data) == 0 || len(data) > maxFlowChunk {
		return fmt.Errorf("write size must be between 1 and %d", maxFlowChunk)
	}
	if timeout > 0 {
		if err := conn.SetWriteDeadline(time.Now().Add(timeout)); err != nil {
			return err
		}
		defer conn.SetWriteDeadline(time.Time{})
	}
	if network == "udp" {
		n, err := conn.Write(data)
		if err == nil && n != len(data) {
			return io.ErrShortWrite
		}
		return err
	}
	for len(data) > 0 {
		n, err := conn.Write(data)
		if err != nil {
			return err
		}
		if n == 0 {
			return io.ErrShortWrite
		}
		data = data[n:]
	}
	return nil
}

func (c *FlowConnection) Close() error {
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil
	}
	c.closed = true
	conn := c.conn
	c.mu.Unlock()
	return conn.Close()
}
