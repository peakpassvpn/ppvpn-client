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
type NodeSummary struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Protocol    string `json:"protocol"`
	Region      string `json:"region,omitempty"`
	CountryCode string `json:"country_code,omitempty"`
	TCP         bool   `json:"tcp"`
	UDP         bool   `json:"udp"`
}
type rawRequest struct {
	Profile     json.RawMessage `json:"profile"`
	NodeID      string          `json:"node_id"`
	NodeIDs     []string        `json:"node_ids"`
	TimeoutMS   int             `json:"timeout_ms"`
	Concurrency int             `json:"concurrency"`
	Target      string          `json:"target"`
}
