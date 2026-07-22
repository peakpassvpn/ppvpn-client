// Package mobile exposes gomobile-friendly primitives. It intentionally uses
// JSON DTOs and never exports sing-box values to Swift, Kotlin, or JNI.
package mobile

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	coreruntime "github.com/peakpassvpn/ppvpn-core/internal/runtime"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/version"
)

type Bridge struct{ core *coreruntime.Core }
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
