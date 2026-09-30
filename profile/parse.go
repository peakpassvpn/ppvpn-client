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
		if removed := rejectRemovedFields(data); removed != nil {
			return nil, removed
		}
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

// rejectRemovedFields turns the strict decoder's generic unknown-field error
// into a coded one for fields that were renamed. shadowsocks.server_key was
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
