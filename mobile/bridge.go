// Package mobile exposes gomobile-friendly primitives. It intentionally uses
// JSON DTOs and never exports sing-box values to Swift, Kotlin, or JNI.
package mobile

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"sync"
	"time"

	coreruntime "github.com/peakpassvpn/ppvpn-core/internal/runtime"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/routing"
	"github.com/peakpassvpn/ppvpn-core/version"
)

type Bridge struct{ core *coreruntime.Core }
type FlowConnection struct {
	mu         sync.Mutex
	connection *coreruntime.FlowConnection
}
type EventHandler interface{ OnEvent(eventJSON string) }
type EventWatcher struct{ cancel context.CancelFunc }

func (w *EventWatcher) Close() {
	if w != nil && w.cancel != nil {
		w.cancel()
	}
}

func NewBridge(platformJSON, statePath string) (*Bridge, error) {
	var platform profile.PlatformCapabilities
	if err := json.Unmarshal([]byte(platformJSON), &platform); err != nil {
		return nil, fmt.Errorf("decode platform capabilities: %w", err)
	}
	return &Bridge{core: coreruntime.NewWithLocalProxyState(platform, statePath)}, nil
}
func (b *Bridge) Version() (string, error) { return encode(version.Get()) }
func (b *Bridge) ValidateProfile(profileJSON string) (string, error) {
	p, err := profile.Parse([]byte(profileJSON))
	if err == nil {
		err = profile.Validate(p, time.Now())
	}
	if err != nil {
		return "", safeError(err)
	}
	return `{"valid":true}`, nil
}
func (b *Bridge) ApplyProfile(profileJSON string) (string, error) {
	p, err := profile.Parse([]byte(profileJSON))
	if err != nil {
		return "", safeError(err)
	}
	applied, err := b.core.ApplyProfile(p, time.Now())
	if err != nil {
		return "", safeError(err)
	}
	return encode(map[string]bool{"applied": applied})
}
func (b *Bridge) Start() error            { return safeError(b.core.Start()) }
func (b *Bridge) Stop() error             { return safeError(b.core.Stop()) }
func (b *Bridge) Status() (string, error) { return encode(b.core.Status()) }
func (b *Bridge) ListNodes() (string, error) {
	type summary struct {
		ID       string `json:"id"`
		Name     string `json:"name"`
		Protocol string `json:"protocol"`
		TCP      bool   `json:"tcp"`
		UDP      bool   `json:"udp"`
	}
	nodes := b.core.Nodes()
	result := make([]summary, len(nodes))
	for i, n := range nodes {
		result[i] = summary{ID: n.ID, Name: n.Name, Protocol: string(n.Protocol), TCP: n.Capabilities.TCP, UDP: n.Capabilities.UDP}
	}
	return encode(result)
}
func (b *Bridge) SelectNode(nodeID string) error       { return safeError(b.core.SelectNode(nodeID)) }
func (b *Bridge) LocalProxyEndpoints() (string, error) { return encode(b.core.LocalProxyEndpoints()) }
func (b *Bridge) LocalProxyMetadata() (string, error) {
	return encode(b.core.LocalProxyMetadata())
}
func (b *Bridge) LocalProxyCredential(nodeID string) (string, error) {
	credential, err := b.core.LocalProxyCredential(nodeID)
	if err != nil {
		return "", safeError(err)
	}
	return encode(credential)
}
func (b *Bridge) ClassifyFlow(flowJSON string) (string, error) {
	flow, err := decodeFlow(flowJSON)
	if err != nil {
		return "", safeError(err)
	}
	decision, err := b.core.ClassifyFlow(flow)
	if err != nil {
		return "", safeError(err)
	}
	return encode(decision)
}
func (b *Bridge) OpenFlow(flowJSON, decisionJSON string, timeoutMS int) (*FlowConnection, error) {
	flow, err := decodeFlow(flowJSON)
	if err != nil {
		return nil, safeError(err)
	}
	decision, err := decodeDecision(decisionJSON)
	if err != nil {
		return nil, safeError(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), mobileDuration(timeoutMS, 15*time.Second))
	defer cancel()
	connection, err := b.core.OpenFlow(ctx, flow, decision)
	if err != nil {
		return nil, safeError(err)
	}
	return &FlowConnection{connection: connection}, nil
}
func (b *Bridge) SystemProxyEndpoints() (string, error) {
	endpoints, err := b.core.SystemProxyEndpoints()
	if err != nil {
		return "", safeError(err)
	}
	return encode(endpoints)
}
func (b *Bridge) Traffic() (string, error)     { return encode(b.core.Traffic()) }
func (b *Bridge) Connections() (string, error) { return encode(b.core.Connections()) }
func (b *Bridge) ProbeEntrances(timeoutMS, concurrency int) (string, error) {
	results, err := b.core.ProbeEntrances(context.Background(), mobileDuration(timeoutMS, 5*time.Second), concurrency)
	if err != nil {
		return "", safeError(err)
	}
	return encode(results)
}
func (b *Bridge) ProbeAvailability(nodeID, target string, timeoutMS int) (string, error) {
	result, err := b.core.ProbeAvailability(context.Background(), nodeID, target, mobileDuration(timeoutMS, 10*time.Second))
	if err != nil {
		return "", safeError(err)
	}
	return encode(result)
}
func (b *Bridge) WatchEvents(handler EventHandler) (*EventWatcher, error) {
	if handler == nil {
		return nil, fmt.Errorf("event handler is required")
	}
	ctx, cancel := context.WithCancel(context.Background())
	events := b.core.Subscribe(ctx, 64)
	go func() {
		for event := range events {
			data, err := json.Marshal(event)
			if err == nil {
				handler.OnEvent(string(data))
			}
		}
	}()
	return &EventWatcher{cancel: cancel}, nil
}
func (c *FlowConnection) Network() string {
	if c == nil {
		return ""
	}
	c.mu.Lock()
	connection := c.connection
	c.mu.Unlock()
	if connection == nil {
		return ""
	}
	return connection.Network()
}
func (c *FlowConnection) NodeID() string {
	if c == nil {
		return ""
	}
	c.mu.Lock()
	connection := c.connection
	c.mu.Unlock()
	if connection == nil {
		return ""
	}
	return connection.NodeID()
}
func (c *FlowConnection) Read(maxBytes, timeoutMS int) ([]byte, error) {
	if c == nil {
		return nil, fmt.Errorf("flow connection is closed")
	}
	c.mu.Lock()
	connection := c.connection
	c.mu.Unlock()
	if connection == nil {
		return nil, fmt.Errorf("flow connection is closed")
	}
	data, err := connection.Read(maxBytes, flowIODuration(timeoutMS))
	return data, safeError(err)
}
func (c *FlowConnection) Write(data []byte, timeoutMS int) error {
	if c == nil {
		return fmt.Errorf("flow connection is closed")
	}
	c.mu.Lock()
	connection := c.connection
	c.mu.Unlock()
	if connection == nil {
		return fmt.Errorf("flow connection is closed")
	}
	return safeError(connection.Write(data, flowIODuration(timeoutMS)))
}
func (c *FlowConnection) Close() error {
	if c == nil {
		return nil
	}
	c.mu.Lock()
	connection := c.connection
	c.connection = nil
	c.mu.Unlock()
	if connection == nil {
		return nil
	}
	return safeError(connection.Close())
}
func decodeFlow(value string) (routing.Flow, error) {
	decoder := json.NewDecoder(bytes.NewBufferString(value))
	decoder.DisallowUnknownFields()
	var flow routing.Flow
	if err := decoder.Decode(&flow); err != nil {
		return routing.Flow{}, fmt.Errorf("decode flow: %w", err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		return routing.Flow{}, fmt.Errorf("flow must contain exactly one JSON value")
	}
	return flow, nil
}
func decodeDecision(value string) (routing.Decision, error) {
	decoder := json.NewDecoder(bytes.NewBufferString(value))
	decoder.DisallowUnknownFields()
	var decision routing.Decision
	if err := decoder.Decode(&decision); err != nil {
		return routing.Decision{}, fmt.Errorf("decode flow decision: %w", err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		return routing.Decision{}, fmt.Errorf("flow decision must contain exactly one JSON value")
	}
	return decision, nil
}
func encode(value any) (string, error) { data, err := json.Marshal(value); return string(data), err }
func safeError(err error) error {
	if err == nil {
		return nil
	}
	if validation, ok := err.(*profile.ValidationError); ok {
		return fmt.Errorf("%s: %s (%s)", validation.Code, validation.Message, validation.Field)
	}
	return fmt.Errorf("core operation failed")
}
func mobileDuration(ms int, fallback time.Duration) time.Duration {
	if ms <= 0 {
		return fallback
	}
	if ms > 120000 {
		ms = 120000
	}
	return time.Duration(ms) * time.Millisecond
}
func flowIODuration(ms int) time.Duration {
	if ms <= 0 {
		return 0
	}
	if ms > 120000 {
		ms = 120000
	}
	return time.Duration(ms) * time.Millisecond
}
