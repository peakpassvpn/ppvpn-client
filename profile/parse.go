package profile

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
)

// Parse decodes a proxy profile. Unknown fields are ignored so a backend can
// add optional fields without breaking clients that shipped earlier; missing
// or invalid required fields still fail (here or in Validate). Fields that
// were removed or renamed are rejected with a coded error, because silently
// ignoring them would hide a backend sending the old shape.
func Parse(data []byte) (*Profile, error) {
	if removed := rejectRemovedFields(data); removed != nil {
		return nil, removed
	}
	dec := json.NewDecoder(bytes.NewReader(data))
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

// rejectRemovedFields rejects fields that were renamed or removed, with a
// coded error. shadowsocks.server_key was
// replaced by the SIP022 pair identity_keys (server iPSKs) + user_key (uPSK);
// a backend still sending it had the two roles swapped.
func rejectRemovedFields(data []byte) error {
	var shape struct {
		Nodes []struct {
			Ingresses []struct {
				Credentials struct {
					Shadowsocks map[string]json.RawMessage `json:"shadowsocks"`
				} `json:"credentials"`
			} `json:"ingresses"`
		} `json:"nodes"`
	}
	if json.Unmarshal(data, &shape) != nil {
		return nil
	}
	for i, n := range shape.Nodes {
		for j, in := range n.Ingresses {
			if _, ok := in.Credentials.Shadowsocks["server_key"]; ok {
				return invalid("SHADOWSOCKS_SERVER_KEY_REMOVED", fmt.Sprintf("nodes[%d].ingresses[%d].credentials.shadowsocks.server_key", i, j), "server_key was removed; send the server iPSKs as identity_keys and the user uPSK as user_key")
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
