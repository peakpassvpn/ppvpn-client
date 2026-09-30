package profile

import (
	"errors"
	"strings"
	"testing"
	"time"
)

const testSHA = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

func ruleSetProfile() *Profile {
	p := validProfile(ProtocolShadowsocks)
	p.Routing.RuleSets = []RuleSet{{ID: "cn-ip", URL: "https://api.example.com/api/v1/proxy-profile/rule-sets/cn-ip.srs", SHA256: testSHA, UpdateIntervalSeconds: 86400}}
	p.Routing.Rules = []RoutingRule{{ID: "geoip-cn", Match: RoutingMatch{RuleSetIDs: []string{"cn-ip"}}, Action: RoutingAction{Type: "direct"}}}
	return p
}

func TestRuleSetValidation(t *testing.T) {
	if code := validationCode(t, ruleSetProfile()); code != "" {
		t.Fatalf("valid rule set profile rejected: %s", code)
	}
	cases := map[string]struct {
		mutate func(*Profile)
		code   string
	}{
		"empty id":         {func(p *Profile) { p.Routing.RuleSets[0].ID = "" }, "RULE_SET_ID_INVALID"},
		"duplicate id":     {func(p *Profile) { p.Routing.RuleSets = append(p.Routing.RuleSets, p.Routing.RuleSets[0]) }, "RULE_SET_ID_DUPLICATE"},
		"http":             {func(p *Profile) { p.Routing.RuleSets[0].URL = "http://api.example.com/x.srs" }, "RULE_SET_URL_INVALID"},
		"relative":         {func(p *Profile) { p.Routing.RuleSets[0].URL = "/x.srs" }, "RULE_SET_URL_INVALID"},
		"userinfo":         {func(p *Profile) { p.Routing.RuleSets[0].URL = "https://u:p@api.example.com/x.srs" }, "RULE_SET_URL_INVALID"},
		"short sha":        {func(p *Profile) { p.Routing.RuleSets[0].SHA256 = "abcd" }, "RULE_SET_SHA256_INVALID"},
		"non-hex sha":      {func(p *Profile) { p.Routing.RuleSets[0].SHA256 = strings.Repeat("z", 64) }, "RULE_SET_SHA256_INVALID"},
		"missing sha":      {func(p *Profile) { p.Routing.RuleSets[0].SHA256 = "" }, "RULE_SET_SHA256_INVALID"},
		"negative":         {func(p *Profile) { p.Routing.RuleSets[0].UpdateIntervalSeconds = -1 }, "RULE_SET_INTERVAL_INVALID"},
		"unknown id":       {func(p *Profile) { p.Routing.Rules[0].Match.RuleSetIDs = []string{"geosite-cn"} }, "RULE_SET_NOT_FOUND"},
		"duplicate ref":    {func(p *Profile) { p.Routing.Rules[0].Match.RuleSetIDs = []string{"cn-ip", "cn-ip"} }, "RULE_SET_REF_DUPLICATE"},
		"empty match":      {func(p *Profile) { p.Routing.Rules[0].Match.RuleSetIDs = nil }, "RULE_MATCH_EMPTY"},
		"too many":         {func(p *Profile) { p.Routing.RuleSets = make([]RuleSet, MaxRuleSets+1) }, "RULE_SET_COUNT_INVALID"},
		"unreferenced set": {func(p *Profile) { p.Routing.Rules = nil }, ""},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			p := ruleSetProfile()
			tc.mutate(p)
			if code := validationCode(t, p); code != tc.code {
				t.Fatalf("code = %q, want %q", code, tc.code)
			}
		})
	}
}

func TestRuleSetHostPinning(t *testing.T) {
	code := func(err error) string {
		var ve *ValidationError
		if err == nil {
			return ""
		}
		if !errors.As(err, &ve) {
			t.Fatalf("unstructured error: %v", err)
		}
		return ve.Code
	}
	p := ruleSetProfile()
	for _, allowed := range [][]string{{"api.example.com"}, {"API.example.com:443"}, {"other.example", "api.example.com"}} {
		if c := code(ValidateRuleSetHosts(p, allowed)); c != "" {
			t.Fatalf("%v: %s", allowed, c)
		}
	}
	for _, allowed := range [][]string{nil, {"example.com"}, {"api.example.com:8443"}, {"evil.api.example.com"}} {
		if c := code(ValidateRuleSetHosts(p, allowed)); c != "RULE_SET_HOST_NOT_ALLOWED" {
			t.Fatalf("%v: code %q", allowed, c)
		}
	}
	if c := code(ValidateRuleSetHosts(p, []string{"https://api.example.com/"})); c != "RULE_SET_HOSTS_INVALID" {
		t.Fatalf("malformed allowed host: %q", c)
	}
	p.Routing.RuleSets[0].URL = "https://api.example.com:8443/x.srs"
	if c := code(ValidateRuleSetHosts(p, []string{"api.example.com:8443"})); c != "" {
		t.Fatalf("explicit port: %s", c)
	}
	p.Routing.RuleSets[0].URL = "https://[2001:DB8::1]/x.srs"
	if c := code(ValidateRuleSetHosts(p, []string{"[2001:db8::1]:443"})); c != "" {
		t.Fatalf("ipv6: %s", c)
	}
}

func TestRuleSetUpdateIntervalClamp(t *testing.T) {
	for seconds, want := range map[int64]time.Duration{
		0: 24 * time.Hour, 60: time.Hour, 7200: 2 * time.Hour, 86400 * 30: 7 * 24 * time.Hour, 1 << 62: 7 * 24 * time.Hour,
	} {
		if got := (RuleSet{UpdateIntervalSeconds: seconds}).UpdateInterval(); got != want {
			t.Errorf("%d: %s, want %s", seconds, got, want)
		}
	}
}
