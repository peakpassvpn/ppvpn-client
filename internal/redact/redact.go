package redact

import (
	"encoding/json"
	"regexp"
	"strings"
)

const Hidden = "[REDACTED]"

var sensitive = map[string]bool{"password": true, "server_key": true, "identity_keys": true, "uuid": true, "public_key": true, "short_id": true, "token": true, "secret": true, "authorization": true}

func JSON(data []byte) []byte {
	var value any
	if json.Unmarshal(data, &value) != nil {
		return []byte(Hidden)
	}
	walk(value)
	out, err := json.Marshal(value)
	if err != nil {
		return []byte(Hidden)
	}
	return out
}
func walk(v any) {
	switch x := v.(type) {
	case map[string]any:
		for k, child := range x {
			if sensitive[strings.ToLower(k)] {
				x[k] = Hidden
			} else {
				walk(child)
			}
		}
	case []any:
		for _, child := range x {
			walk(child)
		}
	}
}

var proxyAuth = regexp.MustCompile(`(?i)(https?|socks5)://[^/@\s]+@`)

func Text(s string) string { return proxyAuth.ReplaceAllString(s, "$1://"+Hidden+"@") }
