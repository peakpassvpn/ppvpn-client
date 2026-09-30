package runtime

import (
	"errors"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

// ErrIngressNotFound rejects a pin to an endpoint_key the node does not have.
var ErrIngressNotFound = errors.New("ingress not found")

// groupEngine exposes a multi-ingress node's failover group.
type groupEngine interface {
	ingressGroup(nodeTag string) (*failover.Group, bool)
}

// NodeStatus is one node's ingress state: the pin (nil in automatic
// selection) and, while the core runs, each ingress's health.
type NodeStatus struct {
	NodeID            string          `json:"node_id"`
	PinnedEndpointKey *string         `json:"pinned_endpoint_key"`
	Ingresses         []IngressHealth `json:"ingresses"`
}

// IngressHealth is one ingress of a node. Healthy, LastCheckAt and
// ConsecutiveFailures come from the failover group's checks: they are
// omitted for a single-ingress node and while the core is stopped, and
// LastCheckAt stays omitted until the first check (checks run only while the
// node carries traffic). Active marks the ingress carrying the node's latest
// connection.
type IngressHealth struct {
	EndpointKey         string     `json:"endpoint_key"`
	Role                string     `json:"role"`
	Label               string     `json:"label,omitempty"`
	Healthy             *bool      `json:"healthy,omitempty"`
	LastCheckAt         *time.Time `json:"last_check_at,omitempty"`
	ConsecutiveFailures int        `json:"consecutive_failures"`
	Active              bool       `json:"active"`
}

// PinIngress pins a node to one ingress (by endpoint_key) or, with an empty
// key, returns it to automatic failover. It takes effect on the running
// engine at once, without a rebuild or a new revision, and survives applies
// while the node keeps that ingress. Pins are not persisted.
func (c *Core) PinIngress(nodeID, endpointKey string) error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.Lock()
	if c.active == nil {
		c.mu.Unlock()
		return ErrProfileNotApplied
	}
	node, ok := findNode(c.active, nodeID)
	if !ok {
		c.mu.Unlock()
		return ErrNodeNotFound
	}
	if endpointKey != "" && !hasIngress(node, endpointKey) {
		c.mu.Unlock()
		return ErrIngressNotFound
	}
	if c.pins == nil {
		c.pins = map[string]string{}
	}
	if endpointKey == "" {
		delete(c.pins, nodeID)
	} else {
		c.pins[nodeID] = endpointKey
	}
	instance, built := c.engine, c.built
	c.mu.Unlock()
	if err := applyPin(instance, built, nodeID, endpointKey); err != nil {
		return err
	}
	c.emit(Event{Type: EventNodeIngressPinned, At: time.Now(), NodeID: nodeID, EndpointKey: endpointKey})
	return nil
}

// applyPin sets one node's pin on a running engine; a single-ingress node
// has no group and nothing to do.
func applyPin(instance engine, built *config.BuildResult, nodeID, endpointKey string) error {
	source, ok := instance.(groupEngine)
	if !ok || built == nil {
		return nil
	}
	group, ok := source.ingressGroup(built.NodeTags[nodeID])
	if !ok {
		return nil
	}
	tag := ""
	if endpointKey != "" {
		tag = memberTag(built, nodeID, endpointKey)
	}
	return group.Pin(tag)
}

// applyPins sets every pin on an engine built from built, before it starts
// (the groups keep a pin until their members exist), so its first
// connection already honours the pins. A pin to an ingress built no longer
// has is left out; prunePinsLocked drops it when the profile is committed.
// The caller holds the operation lock, not c.mu.
func (c *Core) applyPins(instance engine, built *config.BuildResult) {
	c.mu.RLock()
	pins := make(map[string]string, len(c.pins))
	for node, key := range c.pins {
		pins[node] = key
	}
	c.mu.RUnlock()
	for node, key := range pins {
		if memberTag(built, node, key) != "" {
			_ = applyPin(instance, built, node, key)
		}
	}
}

// prunePinsLocked drops pins whose node or ingress the new profile no longer
// has and reports each. The caller holds c.mu.
func (c *Core) prunePinsLocked(p *profile.Profile, now time.Time) []Event {
	var events []Event
	for nodeID, key := range c.pins {
		if node, ok := findNode(p, nodeID); ok && hasIngress(node, key) {
			continue
		}
		delete(c.pins, nodeID)
		events = append(events, Event{Type: EventNodeIngressPinCleared, At: now, Revision: p.Revision, NodeID: nodeID, EndpointKey: key})
	}
	return events
}

func hasIngress(node profile.Node, endpointKey string) bool {
	for _, ingress := range node.Ingresses {
		if ingress.EndpointKey == endpointKey {
			return true
		}
	}
	return false
}

// memberTag finds the failover member outbound of nodeID's ingress.
func memberTag(built *config.BuildResult, nodeID, endpointKey string) string {
	for tag, key := range built.IngressKeys {
		if key == endpointKey && built.OutboundNodes[tag] == nodeID && tag != built.NodeTags[nodeID] {
			return tag
		}
	}
	return ""
}

// nodeStatusesLocked reports every node's pin and ingress health. The caller
// holds c.mu (read).
func (c *Core) nodeStatusesLocked() []NodeStatus {
	if c.active == nil {
		return nil
	}
	source, _ := c.engine.(groupEngine)
	out := make([]NodeStatus, 0, len(c.active.Nodes))
	for _, node := range c.active.Nodes {
		status := NodeStatus{NodeID: node.ID, Ingresses: make([]IngressHealth, 0, len(node.Ingresses))}
		if key, ok := c.pins[node.ID]; ok {
			status.PinnedEndpointKey = &key
		}
		var members map[string]failover.MemberStatus
		current := ""
		if source != nil && c.built != nil {
			if group, ok := source.ingressGroup(c.built.NodeTags[node.ID]); ok {
				members = map[string]failover.MemberStatus{}
				for _, m := range group.Members() {
					members[c.built.IngressKeys[m.Tag]] = m
				}
				current = c.built.IngressKeys[group.Active().Current]
			}
		}
		for _, ingress := range node.Ingresses {
			health := IngressHealth{EndpointKey: ingress.EndpointKey, Role: string(ingress.Role), Label: ingress.DisplayLabel(), Active: ingress.EndpointKey == current}
			if m, ok := members[ingress.EndpointKey]; ok {
				healthy := m.Healthy
				health.Healthy, health.ConsecutiveFailures = &healthy, m.ConsecutiveFailures
				if !m.LastCheck.IsZero() {
					at := m.LastCheck.UTC()
					health.LastCheckAt = &at
				}
			}
			status.Ingresses = append(status.Ingresses, health)
		}
		out = append(out, status)
	}
	return out
}
