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
	return &p, nil
}

func ensureEOF(dec *json.Decoder) error {
	var extra any
	if err := dec.Decode(&extra); err != io.EOF {
		return fmt.Errorf("profile must contain exactly one JSON value")
	}
	return nil
}
