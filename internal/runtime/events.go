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
	// EventNodeIngressPinned: the host pinned a node to EndpointKey, or
	// returned it to automatic failover (EndpointKey empty).
	EventNodeIngressPinned EventType = "NodeIngressPinned"
	// EventNodeIngressPinCleared: an applied profile no longer has the
	// pinned node or ingress (EndpointKey), so the pin was dropped.
	EventNodeIngressPinCleared EventType = "NodeIngressPinCleared"
	// EventSystemProxyChanged carries "enabled" or "disabled" in Message.
	EventSystemProxyChanged EventType = "SystemProxyChanged"
	// EventRuleSetChanged: a rule set changed state. RuleSetID names it,
	// Message carries the new state (ready, stale or unavailable) and Code
	// the error code while it is not ready.
	EventRuleSetChanged EventType = "RuleSetChanged"
	// EventKernelSwitched: an apply took effect without closing listeners.
	// The new profile was applied to the connections of every replaced
	// kernel still draining: ClosedConnections were closed because it no
	// longer allows them, KeptConnections keep draining. DrainingKernels
	// counts the kernels draining after the switch, the one just replaced
	// included.
	EventKernelSwitched EventType = "KernelSwitched"
	// EventKernelDrained: a replaced kernel was closed; Code is "idle" (no
	// connections left) or "deadline" (ClosedConnections were still open).
	EventKernelDrained EventType = "KernelDrained"
	// EventNetworkChanged: the default interface the core binds outbound
	// sockets to changed (only where the core watches it: the desktop TUN).
	// HasDefaultInterface is false when none is left (offline); otherwise
	// InterfaceName and InterfaceIndex name the new one.
	EventNetworkChanged EventType = "NetworkChanged"
)

type Event struct {
	Type     EventType `json:"type"`
	At       time.Time `json:"at"`
	Revision string    `json:"revision,omitempty"`
	NodeID   string    `json:"node_id,omitempty"`
	Message  string    `json:"message,omitempty"`
	// EndpointKey is set on NodeIngressSwitched, NodeIngressPinned and
	// NodeIngressPinCleared; PreviousEndpointKey on NodeIngressSwitched.
	EndpointKey         string `json:"endpoint_key,omitempty"`
	PreviousEndpointKey string `json:"previous_endpoint_key,omitempty"`
	RuleSetID           string `json:"rule_set_id,omitempty"`
	Code                string `json:"code,omitempty"`
	// ClosedConnections and KeptConnections are set on KernelSwitched and
	// KernelDrained.
	ClosedConnections int `json:"closed_connections,omitempty"`
	KeptConnections   int `json:"kept_connections,omitempty"`
	// DrainingKernels is set on KernelSwitched.
	DrainingKernels int `json:"draining_kernels,omitempty"`
	// Set on NetworkChanged.
	InterfaceName       string `json:"interface_name,omitempty"`
	InterfaceIndex      int    `json:"interface_index,omitempty"`
	HasDefaultInterface *bool  `json:"has_default_interface,omitempty"`
}

type eventBus struct {
	subscribers map[uint64]chan Event
	next        uint64
}

func newEventBus() *eventBus { return &eventBus{subscribers: map[uint64]chan Event{}} }
