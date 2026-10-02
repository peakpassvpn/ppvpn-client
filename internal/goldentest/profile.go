// Package goldentest holds what the golden-file tests share
// (testdata/golden): profiles given as a base file plus an RFC 6902 patch.
// Only tests import it.
package goldentest

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// ProfileRef is a profile as a base file (relative to the golden directory)
// with an RFC 6902 JSON Patch applied.
type ProfileRef struct {
	Base  string            `json:"base"`
	Patch []json.RawMessage `json:"patch,omitempty"`
}

// ResolveProfile reads ref.Base under dir and applies ref.Patch.
func ResolveProfile(dir string, ref ProfileRef) (json.RawMessage, error) {
	raw, err := os.ReadFile(filepath.Join(dir, ref.Base))
	if err != nil {
		return nil, err
	}
	var doc any
	if err := json.Unmarshal(raw, &doc); err != nil {
		return nil, err
	}
	for _, op := range ref.Patch {
		if doc, err = ApplyPatchOp(doc, op); err != nil {
			return nil, err
		}
	}
	return json.Marshal(doc)
}

// ApplyPatchOp applies one RFC 6902 operation (add, remove, replace).
func ApplyPatchOp(doc any, raw json.RawMessage) (any, error) {
	var op struct {
		Op    string          `json:"op"`
		Path  string          `json:"path"`
		Value json.RawMessage `json:"value"`
	}
	if err := json.Unmarshal(raw, &op); err != nil {
		return nil, err
	}
	var value any
	if op.Op != "remove" {
		if err := json.Unmarshal(op.Value, &value); err != nil {
			return nil, fmt.Errorf("%s %s: value: %w", op.Op, op.Path, err)
		}
	}
	tokens := strings.Split(op.Path, "/")[1:]
	for i := range tokens {
		tokens[i] = strings.ReplaceAll(strings.ReplaceAll(tokens[i], "~1", "/"), "~0", "~")
	}
	return patchAt(doc, tokens, op.Op, value)
}

func patchAt(node any, tokens []string, op string, value any) (any, error) {
	if len(tokens) == 0 {
		return value, nil
	}
	last := len(tokens) == 1
	switch container := node.(type) {
	case map[string]any:
		key := tokens[0]
		if last {
			switch op {
			case "remove":
				if _, ok := container[key]; !ok {
					return nil, fmt.Errorf("remove: no member %q", key)
				}
				delete(container, key)
			case "replace":
				if _, ok := container[key]; !ok {
					return nil, fmt.Errorf("replace: no member %q", key)
				}
				container[key] = value
			default:
				container[key] = value
			}
			return container, nil
		}
		child, ok := container[key]
		if !ok {
			return nil, fmt.Errorf("no member %q", key)
		}
		updated, err := patchAt(child, tokens[1:], op, value)
		container[key] = updated
		return container, err
	case []any:
		if last && tokens[0] == "-" && op == "add" {
			return append(container, value), nil
		}
		index, err := strconv.Atoi(tokens[0])
		if err != nil || index < 0 || index > len(container) || (index == len(container) && !(last && op == "add")) {
			return nil, fmt.Errorf("bad index %q", tokens[0])
		}
		if last {
			switch op {
			case "remove":
				return append(container[:index:index], container[index+1:]...), nil
			case "replace":
				container[index] = value
				return container, nil
			default:
				out := append(container[:index:index], value)
				return append(out, container[index:]...), nil
			}
		}
		updated, err := patchAt(container[index], tokens[1:], op, value)
		container[index] = updated
		return container, err
	}
	return nil, fmt.Errorf("path through a scalar at %q", tokens[0])
}
