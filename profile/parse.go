package profile

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
)

func Parse(data []byte) (*Profile, error) {
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.DisallowUnknownFields()
	var p Profile
	if err := dec.Decode(&p); err != nil {
		return nil, fmt.Errorf("decode profile: %w", err)
	}
	if err := ensureEOF(dec); err != nil {
		return nil, err
	}
	if err := requirePresence(data); err != nil {
		return nil, err
	}
	return &p, nil
}

// requirePresence rejects required fields whose zero value is a valid value
// and therefore cannot be distinguished from absence after decoding into
// Profile. replica_ordinal is the only such field: 0 is legal but must be sent.
func requirePresence(data []byte) error {
	var shape struct {
		Nodes []struct {
			Ingresses []struct {
				ReplicaOrdinal *json.RawMessage `json:"replica_ordinal"`
			} `json:"ingresses"`
		} `json:"nodes"`
	}
	if err := json.Unmarshal(data, &shape); err != nil {
		return fmt.Errorf("decode profile: %w", err)
	}
	for i, n := range shape.Nodes {
		for j, in := range n.Ingresses {
			if in.ReplicaOrdinal == nil || string(*in.ReplicaOrdinal) == "null" {
				return invalid("FIELD_REQUIRED", fmt.Sprintf("nodes[%d].ingresses[%d].replica_ordinal", i, j), "replica ordinal is required")
			}
		}
	}
	return nil
}

func ensureEOF(dec *json.Decoder) error {
	var extra any
	if err := dec.Decode(&extra); err != io.EOF {
		return fmt.Errorf("profile must contain exactly one JSON value")
	}
	return nil
}
