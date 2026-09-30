package api

import "encoding/json"

type Envelope struct {
	RequestID string `json:"request_id"`
	OK        bool   `json:"ok"`
	Data      any    `json:"data,omitempty"`
	Error     *Error `json:"error,omitempty"`
}
type Error struct {
	Code      string `json:"code"`
	Message   string `json:"message"`
	Field     string `json:"field,omitempty"`
	Retryable bool   `json:"retryable"`
}

// NodeSummary describes a logical node without credentials or addresses.
// Protocol is the primary (first) ingress protocol; Ingresses are listed in
// failover order.
type NodeSummary struct {
	ID         string           `json:"id"`
	Name       string           `json:"name"`
	EntryKey   string           `json:"entry_key"`
	EntryLabel string           `json:"entry_label,omitempty"`
	Protocol   string           `json:"protocol"`
	Region     string           `json:"region,omitempty"`
	TCP        bool             `json:"tcp"`
	UDP        bool             `json:"udp"`
	Ingresses  []IngressSummary `json:"ingresses"`
}
type IngressSummary struct {
	EndpointKey    string `json:"endpoint_key"`
	Label          string `json:"label,omitempty"`
	ReplicaOrdinal int    `json:"replica_ordinal"`
	Role           string `json:"role"`
	Protocol       string `json:"protocol"`
}
type rawRequest struct {
	Profile     json.RawMessage `json:"profile"`
	NodeID      string          `json:"node_id"`
	NodeIDs     []string        `json:"node_ids"`
	TimeoutMS   int             `json:"timeout_ms"`
	Concurrency int             `json:"concurrency"`
	Target      string          `json:"target"`
	Method      string          `json:"method"`
	Enabled     *bool           `json:"enabled"`
	// AllowedRuleSetHosts (apply-profile, validate-profile) pins rule set
	// URLs to the authorities of the API the profile came from.
	AllowedRuleSetHosts []string `json:"allowed_rule_set_hosts"`
	// RoutingMode (apply-profile, validate-profile): "rules" (default) or
	// "global".
	RoutingMode string `json:"routing_mode"`
}
