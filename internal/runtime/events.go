package runtime

import (
	"time"

	"github.com/peakpassvpn/ppvpn-core/systemproxy"
)

type EventType string

const (
	EventProfileApplied             EventType = "ProfileApplied"
	EventNodeEndpointChanged        EventType = "NodeEndpointChanged"
	EventReloadFailed               EventType = "ReloadFailed"
	EventNodeSelected               EventType = "NodeSelected"
	EventCoreStarted                EventType = "CoreStarted"
	EventCoreStopped                EventType = "CoreStopped"
	EventEntranceProbed             EventType = "EntranceProbed"
	EventAvailabilityProbed         EventType = "AvailabilityProbed"
	EventSystemProxyEndpointReady   EventType = "SystemProxyEndpointReady"
	EventSystemProxyEndpointStopped EventType = "SystemProxyEndpointStopped"
)

type Event struct {
	Type        EventType              `json:"type"`
	At          time.Time              `json:"at"`
	Revision    string                 `json:"revision,omitempty"`
	NodeID      string                 `json:"node_id,omitempty"`
	Message     string                 `json:"message,omitempty"`
	SystemProxy *systemproxy.Endpoints `json:"system_proxy,omitempty"`
}

type eventBus struct {
	subscribers map[uint64]chan Event
	next        uint64
}

func newEventBus() *eventBus { return &eventBus{subscribers: map[uint64]chan Event{}} }
