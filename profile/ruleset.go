package profile

import (
	"encoding/hex"
	"fmt"
	"net"
	"net/url"
	"strings"
	"time"
)

const (
	// MaxRuleSets bounds how many rule sets one profile may declare.
	MaxRuleSets = 32
	// DefaultRuleSetUpdateInterval applies when update_interval_seconds is
	// absent or 0.
	DefaultRuleSetUpdateInterval = 24 * time.Hour
	// MinRuleSetUpdateInterval and MaxRuleSetUpdateInterval clamp the
	// profile's update_interval_seconds.
	MinRuleSetUpdateInterval = time.Hour
	MaxRuleSetUpdateInterval = 7 * 24 * time.Hour
)

// UpdateInterval is the refresh interval: update_interval_seconds clamped to
// [1h, 7d], or 24h when absent.
func (r RuleSet) UpdateInterval() time.Duration {
	if r.UpdateIntervalSeconds <= 0 {
		return DefaultRuleSetUpdateInterval
	}
	interval := MaxRuleSetUpdateInterval
	if r.UpdateIntervalSeconds < int64(MaxRuleSetUpdateInterval/time.Second) {
		interval = time.Duration(r.UpdateIntervalSeconds) * time.Second
	}
	return min(max(interval, MinRuleSetUpdateInterval), MaxRuleSetUpdateInterval)
}

// SHA256Hex is the digest in lowercase hex.
func (r RuleSet) SHA256Hex() string { return strings.ToLower(r.SHA256) }

// HasAddressMatch reports whether the match has an address matcher other
// than rule sets.
func (m RoutingMatch) HasAddressMatch() bool {
	return len(m.Domains) > 0 || len(m.DomainSuffixes) > 0 || len(m.IPCIDRs) > 0 || m.IPIsPrivate
}

func validateRuleSets(sets []RuleSet) (map[string]bool, error) {
	if len(sets) > MaxRuleSets {
		return nil, invalid("RULE_SET_COUNT_INVALID", "routing.rule_sets", fmt.Sprintf("at most %d rule sets are supported", MaxRuleSets))
	}
	ids := make(map[string]bool, len(sets))
	for i, set := range sets {
		base := fmt.Sprintf("routing.rule_sets[%d]", i)
		if !stableID.MatchString(set.ID) {
			return nil, invalid("RULE_SET_ID_INVALID", base+".id", "rule set id is not stable or valid")
		}
		if ids[set.ID] {
			return nil, invalid("RULE_SET_ID_DUPLICATE", base+".id", "rule set id must be unique")
		}
		ids[set.ID] = true
		if _, err := RuleSetHost(set.URL); err != nil {
			return nil, invalid("RULE_SET_URL_INVALID", base+".url", err.Error())
		}
		if len(set.SHA256) != 64 || !hexPattern.MatchString(set.SHA256) {
			return nil, invalid("RULE_SET_SHA256_INVALID", base+".sha256", "sha256 must be 64 hexadecimal characters")
		}
		if _, err := hex.DecodeString(set.SHA256); err != nil {
			return nil, invalid("RULE_SET_SHA256_INVALID", base+".sha256", "sha256 must be 64 hexadecimal characters")
		}
		if set.UpdateIntervalSeconds < 0 {
			return nil, invalid("RULE_SET_INTERVAL_INVALID", base+".update_interval_seconds", "update interval must not be negative")
		}
	}
	return ids, nil
}

func validateRuleSetRefs(ids []string, known map[string]bool, field string) error {
	seen := make(map[string]bool, len(ids))
	for i, id := range ids {
		if !known[id] {
			return invalid("RULE_SET_NOT_FOUND", fmt.Sprintf("%s[%d]", field, i), "rule set does not exist")
		}
		if seen[id] {
			return invalid("RULE_SET_REF_DUPLICATE", fmt.Sprintf("%s[%d]", field, i), "rule set must be unique within a rule")
		}
		seen[id] = true
	}
	return nil
}

// RuleSetHost returns the normalized host of an https rule set URL: the
// lowercase authority with a default :443 removed. Userinfo, fragments,
// non-https schemes and relative URLs are rejected.
func RuleSetHost(raw string) (string, error) {
	u, err := url.Parse(raw)
	if err != nil || !u.IsAbs() || u.Opaque != "" {
		return "", fmt.Errorf("url must be an absolute https URL")
	}
	if u.Scheme != "https" {
		return "", fmt.Errorf("url must use https")
	}
	if u.User != nil || u.Fragment != "" {
		return "", fmt.Errorf("url must not contain userinfo or a fragment")
	}
	return NormalizeRuleSetHost(u.Host)
}

// NormalizeRuleSetHost normalizes a host or host:port authority for pinning:
// lowercase, default port 443 removed, IPv6 literals bracketed.
func NormalizeRuleSetHost(authority string) (string, error) {
	if authority == "" || strings.ContainsAny(authority, "/?#@ ") {
		return "", fmt.Errorf("url host is invalid")
	}
	host, port := authority, ""
	if h, p, err := net.SplitHostPort(authority); err == nil {
		host, port = h, p
	} else if strings.HasPrefix(authority, "[") && strings.HasSuffix(authority, "]") {
		host = authority[1 : len(authority)-1]
	}
	if host == "" {
		return "", fmt.Errorf("url host is invalid")
	}
	if port != "" {
		var n int
		if _, err := fmt.Sscanf(port, "%d", &n); err != nil || n <= 0 || n > 65535 || fmt.Sprint(n) != port {
			return "", fmt.Errorf("url port is invalid")
		}
	}
	host = strings.ToLower(host)
	if strings.Contains(host, ":") {
		host = "[" + host + "]"
	}
	if port == "" || port == "443" {
		return host, nil
	}
	return host + ":" + port, nil
}

// ValidateRuleSetHosts pins every rule set URL to one of allowed (the
// authorities of the API the host fetched the profile from). It runs in
// addition to Validate when the host supplied allowed_rule_set_hosts.
func ValidateRuleSetHosts(p *Profile, allowed []string) error {
	pinned := make(map[string]bool, len(allowed))
	for _, value := range allowed {
		host, err := NormalizeRuleSetHost(value)
		if err != nil {
			return invalid("RULE_SET_HOSTS_INVALID", "allowed_rule_set_hosts", "allowed rule set host is invalid")
		}
		pinned[host] = true
	}
	for i, set := range p.Routing.RuleSets {
		host, err := RuleSetHost(set.URL)
		if err != nil {
			return invalid("RULE_SET_URL_INVALID", fmt.Sprintf("routing.rule_sets[%d].url", i), err.Error())
		}
		if !pinned[host] {
			return invalid("RULE_SET_HOST_NOT_ALLOWED", fmt.Sprintf("routing.rule_sets[%d].url", i), "rule set host must be the host the profile came from")
		}
	}
	return nil
}
