package config

import (
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
)

func ruleSetRoutingProfile() *profile.Profile {
	p := base(node(profile.ProtocolShadowsocks))
	sha := strings.Repeat("a", 64)
	p.Routing = profile.Routing{
		RuleSets: []profile.RuleSet{
			{ID: "cn-ip", URL: "https://api.example.com/cn-ip.srs", SHA256: sha},
			{ID: "cn-site", URL: "https://api.example.com/cn-site.srs", SHA256: sha},
			{ID: "ads", URL: "https://api.example.com/ads.srs", SHA256: sha},
		},
		Rules: []profile.RoutingRule{
			{ID: "policy", Match: profile.RoutingMatch{RuleSetIDs: []string{"cn-site"}}, Action: profile.RoutingAction{Type: "direct"}},
			{ID: "ads", Match: profile.RoutingMatch{RuleSetIDs: []string{"ads"}, Ports: []uint16{443}}, Action: profile.RoutingAction{Type: "reject"}},
			{ID: "mixed", Match: profile.RoutingMatch{RuleSetIDs: []string{"ads"}, Domains: []string{"blocked.example"}}, Action: profile.RoutingAction{Type: "proxy", Target: "selected"}},
			{ID: "geoip-cn", Match: profile.RoutingMatch{RuleSetIDs: []string{"cn-ip"}}, Action: profile.RoutingAction{Type: "direct"}},
		},
		Final: profile.RoutingAction{Type: "proxy", Target: "selected"},
	}
	return p
}

// profileRules returns the route rules of a non-TUN build without local
// proxies: exactly the profile rules.
func profileRules(result *BuildResult) []option.Rule { return result.Options.Route.Rules }

func TestRuleSetsRenderAsLocalBinaryAndRulesReferenceThem(t *testing.T) {
	files := map[string]RuleSetFile{
		"cn-ip":   {Path: "/state/rule-sets/cn-ip.srs"},
		"cn-site": {Path: "/state/rule-sets/cn-site.srs", MirrorDNS: true},
		"ads":     {Path: "/state/rule-sets/ads.srs", MirrorDNS: true},
	}
	result, err := BuildWithOptions(ruleSetRoutingProfile(), profile.PlatformCapabilities{Platform: "linux"}, BuildOptions{RuleSets: files}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	sets := result.Options.Route.RuleSet
	if len(sets) != 3 {
		t.Fatalf("rule sets: %#v", sets)
	}
	for i, id := range []string{"cn-ip", "cn-site", "ads"} {
		if sets[i].Type != C.RuleSetTypeLocal || sets[i].Format != C.RuleSetFormatBinary || sets[i].Tag != RuleSetTag(id) || sets[i].LocalOptions.Path != files[id].Path {
			t.Fatalf("rule set %d: %#v", i, sets[i])
		}
	}
	rules := profileRules(result)
	if len(rules) != 4 {
		t.Fatalf("profile rules: %#v", rules)
	}
	want := []struct {
		sets    []string
		ports   []uint16
		domains []string
	}{
		{[]string{"rule-set-cn-site"}, nil, nil},
		{[]string{"rule-set-ads"}, []uint16{443}, nil},
		{[]string{"rule-set-ads"}, nil, []string{"blocked.example"}},
		{[]string{"rule-set-cn-ip"}, nil, nil},
	}
	for i, w := range want {
		raw := rules[i].DefaultOptions.RawDefaultRule
		if !slices.Equal(raw.RuleSet, w.sets) || !slices.Equal(raw.Port, w.ports) || !slices.Equal(raw.Domain, w.domains) {
			t.Fatalf("rule %d: %#v", i, raw)
		}
	}
}

// A rule set without a local copy is dropped from every rule: a rule left
// without address matchers disappears, a rule with other matchers keeps them.
func TestUnavailableRuleSetsAreSkipped(t *testing.T) {
	files := map[string]RuleSetFile{"cn-ip": {Path: "/state/rule-sets/cn-ip.srs"}}
	result, err := BuildWithOptions(ruleSetRoutingProfile(), profile.PlatformCapabilities{Platform: "linux"}, BuildOptions{RuleSets: files}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if sets := result.Options.Route.RuleSet; len(sets) != 1 || sets[0].Tag != "rule-set-cn-ip" {
		t.Fatalf("only the available set may be declared: %#v", sets)
	}
	rules := profileRules(result)
	if len(rules) != 2 {
		t.Fatalf("profile rules: %#v", rules)
	}
	if raw := rules[0].DefaultOptions.RawDefaultRule; len(raw.RuleSet) != 0 || !slices.Equal(raw.Domain, []string{"blocked.example"}) {
		t.Fatalf("mixed rule: %#v", raw)
	}
	if raw := rules[1].DefaultOptions.RawDefaultRule; !slices.Equal(raw.RuleSet, []string{"rule-set-cn-ip"}) {
		t.Fatalf("geoip rule: %#v", raw)
	}

	// Without any local copy (render, or a core without state) no rule set
	// is declared at all.
	result, err = Build(ruleSetRoutingProfile(), profile.PlatformCapabilities{Platform: "linux"}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Options.Route.RuleSet) != 0 || len(profileRules(result)) != 1 {
		t.Fatalf("rules without sets: %#v", result.Options.Route)
	}
}

// Domain rule sets are mirrored into DNS like domain matchers: direct to
// dns-local, proxy to dns-remote, reject refused. IP CIDR sets are not.
func TestRuleSetsMirrorIntoTUNDNS(t *testing.T) {
	files := map[string]RuleSetFile{
		"cn-ip":   {Path: "/state/rule-sets/cn-ip.srs"},
		"cn-site": {Path: "/state/rule-sets/cn-site.srs", MirrorDNS: true},
		"ads":     {Path: "/state/rule-sets/ads.srs", MirrorDNS: true},
	}
	result, err := BuildWithOptions(ruleSetRoutingProfile(), profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}}, BuildOptions{RuleSets: files}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	var mirrored []option.DefaultDNSRule
	for _, rule := range result.Options.DNS.Rules {
		if len(rule.DefaultOptions.RuleSet) > 0 {
			mirrored = append(mirrored, rule.DefaultOptions)
		}
	}
	if len(mirrored) != 3 {
		t.Fatalf("mirrored rule-set DNS rules: %#v", result.Options.DNS.Rules)
	}
	if r := mirrored[0]; !slices.Equal(r.RuleSet, []string{"rule-set-cn-site"}) || r.Action != C.RuleActionTypeRoute || r.RouteOptions.Server != DNSLocalTag {
		t.Fatalf("direct set: %#v", r)
	}
	if r := mirrored[1]; !slices.Equal(r.RuleSet, []string{"rule-set-ads"}) || r.Action != C.RuleActionTypeReject {
		t.Fatalf("reject set: %#v", r)
	}
	if r := mirrored[2]; !slices.Equal(r.RuleSet, []string{"rule-set-ads"}) || !slices.Equal(r.Domain, []string{"blocked.example"}) || r.RouteOptions.Server != DNSRemoteTag {
		t.Fatalf("proxy set: %#v", r)
	}
	for _, rule := range result.Options.DNS.Rules {
		if slices.Contains(rule.DefaultOptions.RuleSet, "rule-set-cn-ip") {
			t.Fatalf("IP CIDR set mirrored into DNS: %#v", rule)
		}
	}
}
