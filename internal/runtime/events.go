package runtime

import "time"

type EventType string

const (
	EventProfileApplied      EventType = "ProfileApplied"
	EventNodeEndpointChanged EventType = "NodeEndpointChanged"
	EventReloadFailed        EventType = "ReloadFailed"
	EventNodeSelected        EventType = "NodeSelected"
	EventCoreStarted         EventType = "CoreStarted"
	EventCoreStopped         EventType = "CoreStopped"
	EventEntranceProbed      EventType = "EntranceProbed"
	EventAvailabilityProbed  EventType = "AvailabilityProbed"
	// EventNodeIngressSwitched: a node's failover group moved its traffic to
	// another ingress (to a backup, or back to the primary).
	EventNodeIngressSwitched EventType = "NodeIngressSwitched"
	// EventSystemProxyChanged carries "enabled" or "disabled" in Message.
	EventSystemProxyChanged EventType = "SystemProxyChanged"
)

type Event struct {
	Type     EventType `json:"type"`
	At       time.Time `json:"at"`
	Revision string    `json:"revision,omitempty"`
	NodeID   string    `json:"node_id,omitempty"`
	Message  string    `json:"message,omitempty"`
	// EndpointKey and PreviousEndpointKey are set on NodeIngressSwitched.
	EndpointKey         string `json:"endpoint_key,omitempty"`
	PreviousEndpointKey string `json:"previous_endpoint_key,omitempty"`
}

type eventBus struct {
	subscribers map[uint64]chan Event
	next        uint64
}

func newEventBus() *eventBus { return &eventBus{subscribers: map[uint64]chan Event{}} }
