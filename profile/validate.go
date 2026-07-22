package profile

import (
	"encoding/base64"
	"fmt"
	"net/netip"
	"regexp"
	"strings"
	"time"
)

type ValidationError struct {
	Code, Message, Field string
	Retryable            bool
}

func (e *ValidationError) Error() string {
	return fmt.Sprintf("%s: %s (%s)", e.Code, e.Message, e.Field)
}
func invalid(code, field, message string) error {
	return &ValidationError{Code: code, Field: field, Message: message}
}

var stableID = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`)
var uuidPattern = regexp.MustCompile(`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$`)
var hexPattern = regexp.MustCompile(`^[0-9a-fA-F]*$`)

func Validate(p *Profile, now time.Time) error {
	if p == nil {
		return invalid("PROFILE_REQUIRED", "", "profile is required")
	}
	if p.SchemaVersion != CurrentSchemaVersion {
		return invalid("SCHEMA_UNSUPPORTED", "schema_version", "unsupported profile schema version")
	}
	if strings.TrimSpace(p.Revision) == "" {
		return invalid("FIELD_REQUIRED", "revision", "revision is required")
	}
	if !p.GeneratedAt.IsZero() && !p.ExpiresAt.IsZero() && !p.ExpiresAt.After(p.GeneratedAt) {
		return invalid("TIME_RANGE_INVALID", "expires_at", "expiration must be after generation")
	}
	if !p.ExpiresAt.IsZero() && !now.Before(p.ExpiresAt) {
		return invalid("PROFILE_EXPIRED", "expires_at", "profile has expired")
	}
	if len(p.Nodes) == 0 {
		return invalid("FIELD_REQUIRED", "nodes", "at least one node is required")
	}
	seen := map[string]bool{}
	for i := range p.Nodes {
		n := &p.Nodes[i]
		base := fmt.Sprintf("nodes[%d]", i)
		if !stableID.MatchString(n.ID) {
			return invalid("NODE_ID_INVALID", base+".id", "node id is not stable or valid")
		}
		if seen[n.ID] {
			return invalid("NODE_ID_DUPLICATE", base+".id", "node id must be unique")
		}
		seen[n.ID] = true
		if !validDomain(n.Endpoint.Domain) {
			return invalid("FIELD_REQUIRED", base+".endpoint.domain", "connection domain is required")
		}
		if n.Endpoint.Port == 0 {
			return invalid("PORT_INVALID", base+".endpoint.port", "port must be non-zero")
		}
		ip, err := netip.ParseAddr(n.Endpoint.IP)
		if err != nil || !isPublicUnicast(ip) {
			return invalid("ENTRY_IP_NOT_PUBLIC", base+".endpoint.ip", "entry IP must be a public unicast address")
		}
		if err := validateCredentials(n, base); err != nil {
			return err
		}
		if n.Transport != nil && n.Transport.Type != "" {
			return invalid("TRANSPORT_UNSUPPORTED", base+".transport.type", "transport is not supported in schema version 1")
		}
		if !n.Capabilities.TCP && !n.Capabilities.UDP {
			return invalid("CAPABILITIES_INVALID", base+".capabilities", "at least one network capability is required")
		}
	}
	if !seen[p.Selection.DefaultNodeID] {
		return invalid("DEFAULT_NODE_NOT_FOUND", "selection.default_node_id", "default node does not exist")
	}
	if p.Selection.Mode != "manual" {
		return invalid("SELECTION_MODE_UNSUPPORTED", "selection.mode", "only manual selection is supported")
	}
	return nil
}

func validateCredentials(n *Node, base string) error {
	count := 0
	if n.Credentials.Shadowsocks != nil {
		count++
	}
	if n.Credentials.VLESS != nil {
		count++
	}
	if n.Credentials.AnyTLS != nil {
		count++
	}
	if count != 1 {
		return invalid("CREDENTIALS_INVALID", base+".credentials", "exactly one protocol credential object is required")
	}
	switch n.Protocol {
	case ProtocolShadowsocks:
		c := n.Credentials.Shadowsocks
		if c == nil || c.Method == "" || c.ServerKey == "" {
			return invalid("CREDENTIALS_INVALID", base+".credentials.shadowsocks", "shadowsocks credentials are incomplete")
		}
		keyLength := 0
		switch c.Method {
		case "2022-blake3-aes-128-gcm":
			keyLength = 16
		case "2022-blake3-aes-256-gcm", "2022-blake3-chacha20-poly1305":
			keyLength = 32
		default:
			return invalid("SHADOWSOCKS_METHOD_UNSUPPORTED", base+".credentials.shadowsocks.method", "only Shadowsocks 2022 methods are supported")
		}
		if !validKey(c.ServerKey, keyLength) {
			return invalid("SHADOWSOCKS_KEY_INVALID", base+".credentials.shadowsocks.server_key", "server key has invalid encoding or length")
		}
		for i, key := range c.IdentityKeys {
			if !validKey(key, keyLength) {
				return invalid("CREDENTIALS_INVALID", fmt.Sprintf("%s.credentials.shadowsocks.identity_keys[%d]", base, i), "identity key cannot be empty")
			}
		}
	case ProtocolVLESS:
		c := n.Credentials.VLESS
		if c == nil || !uuidPattern.MatchString(c.UUID) {
			return invalid("CREDENTIALS_INVALID", base+".credentials.vless", "VLESS credentials are incomplete")
		}
		if n.TLS == nil || n.TLS.Reality == nil || n.TLS.ServerName == "" || n.TLS.Reality.PublicKey == "" {
			return invalid("REALITY_REQUIRED", base+".tls.reality", "VLESS REALITY settings are required")
		}
		if n.TLS.ServerName != n.Endpoint.Domain {
			return invalid("TLS_SERVER_NAME_MISMATCH", base+".tls.server_name", "TLS server name must equal endpoint domain")
		}
		if !hexPattern.MatchString(n.TLS.Reality.ShortID) || len(n.TLS.Reality.ShortID)%2 != 0 || len(n.TLS.Reality.ShortID) > 16 {
			return invalid("REALITY_SHORT_ID_INVALID", base+".tls.reality.short_id", "REALITY short ID must be even-length hexadecimal up to 16 characters")
		}
	case ProtocolAnyTLS:
		c := n.Credentials.AnyTLS
		if c == nil || c.Password == "" {
			return invalid("CREDENTIALS_INVALID", base+".credentials.anytls", "AnyTLS credentials are incomplete")
		}
		if n.TLS == nil || n.TLS.ServerName == "" {
			return invalid("TLS_REQUIRED", base+".tls", "AnyTLS TLS settings are required")
		}
		if n.TLS.ServerName != n.Endpoint.Domain {
			return invalid("TLS_SERVER_NAME_MISMATCH", base+".tls.server_name", "TLS server name must equal endpoint domain")
		}
	default:
		return invalid("PROTOCOL_UNSUPPORTED", base+".protocol", "protocol is not supported")
	}
	return nil
}

func validKey(value string, want int) bool {
	decoded, err := base64.StdEncoding.DecodeString(value)
	if err != nil {
		decoded, err = base64.RawStdEncoding.DecodeString(value)
	}
	return err == nil && len(decoded) == want
}

func validDomain(value string) bool {
	if len(value) == 0 || len(value) > 253 || strings.HasSuffix(value, ".") {
		return false
	}
	for _, label := range strings.Split(value, ".") {
		if len(label) == 0 || len(label) > 63 || label[0] == '-' || label[len(label)-1] == '-' {
			return false
		}
		for _, r := range label {
			if !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '-') {
				return false
			}
		}
	}
	return true
}

func isPublicUnicast(ip netip.Addr) bool {
	if !ip.IsValid() || !ip.IsGlobalUnicast() || ip.IsPrivate() || ip.IsLoopback() || ip.IsLinkLocalUnicast() || ip.IsMulticast() || ip.IsUnspecified() {
		return false
	}
	for _, prefix := range reservedPrefixes {
		if prefix.Contains(ip) {
			return false
		}
	}
	return true
}

var reservedPrefixes = mustPrefixes("0.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16", "192.0.0.0/24", "192.0.2.0/24", "198.18.0.0/15", "198.51.100.0/24", "203.0.113.0/24", "224.0.0.0/4", "240.0.0.0/4", "::/128", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8", "2001:db8::/32")

func mustPrefixes(values ...string) []netip.Prefix {
	out := make([]netip.Prefix, len(values))
	for i, v := range values {
		out[i] = netip.MustParsePrefix(v)
	}
	return out
}
