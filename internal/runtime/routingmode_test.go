package runtime

import (
	"crypto/tls"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/rulesets"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

func routingModeProfile(url, sha string) *profile.Profile {
	p := testProfile("modes", "a.example", "8.8.8.8")
	p.Routing.RuleSets = []profile.RuleSet{
		{ID: "cn-site", URL: url, SHA256: sha, UpdateIntervalSeconds: 3600},
	}
	p.Routing.Rules = []profile.RoutingRule{
		{ID: "bypass-private", Baseline: true, Match: profile.RoutingMatch{IPIsPrivate: true}, Action: profile.RoutingAction{Type: "direct"}},
		{ID: "cn", Match: profile.RoutingMatch{RuleSetIDs: []string{"cn-site"}}, Action: profile.RoutingAction{Type: "direct"}},
		{ID: "ads", Match: profile.RoutingMatch{Domains: []string{"ads.example"}}, Action: profile.RoutingAction{Type: "reject"}},
	}
	p.Routing.Final = profile.RoutingAction{Type: "direct"}
	return p
}

func TestEffectiveProfile(t *testing.T) {
	p := routingModeProfile("https://api.example.com/cn-site.srs", "00")
	if got, err := effectiveProfile(p, RoutingModeRules); err != nil || got != p {
		t.Fatalf("rules mode changed the profile: %v", err)
	}
	got, err := effectiveProfile(p, RoutingModeGlobal)
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Routing.Rules) != 1 || got.Routing.Rules[0].ID != "bypass-private" {
		t.Fatalf("rules: %+v", got.Routing.Rules)
	}
	if got.Routing.Final != (profile.RoutingAction{Type: "proxy", Target: "selected"}) || len(got.Routing.RuleSets) != 0 {
		t.Fatalf("final %+v, rule sets %+v", got.Routing.Final, got.Routing.RuleSets)
	}
	// A baseline rule keeps the rule sets it references.
	p.Routing.Rules[1].Baseline = true
	if got, _ = effectiveProfile(p, RoutingModeGlobal); len(got.Routing.RuleSets) != 1 || len(got.Routing.Rules) != 2 {
		t.Fatalf("baseline rule set dropped: %+v", got.Routing)
	}
	if len(p.Routing.Rules) != 3 || p.Routing.Final.Type != "direct" {
		t.Fatal("the applied profile was modified")
	}
	if _, err = ParseRoutingMode("smart"); !errors.Is(err, ErrRoutingModeInvalid) {
		t.Fatalf("invalid mode: %v", err)
	}
}

// The routing mode is part of what an apply changes: switching it re-applies
// the same revision, the global mode renders only baseline rules with the
// selected node as final and never downloads a dropped rule's rule set, and
// reload keeps the mode.
func TestRoutingModeSwitchesWithoutNewRevision(t *testing.T) {
	body := domainRuleSet(t, "cn.example")
	var downloads atomic.Int32
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		downloads.Add(1)
		_, _ = w.Write(body)
	}))
	defer server.Close()
	factory := &optionsFactory{}
	core := newCore(profile.PlatformCapabilities{}, factory.create)
	core.enableRuleSets(t.TempDir(), func(o *rulesets.Options) {
		o.TLSConfig = &tls.Config{RootCAs: server.Client().Transport.(*http.Transport).TLSClientConfig.RootCAs}
	})
	defer core.ruleSets.Close()
	p := routingModeProfile(server.URL+"/cn-site.srs", sha256Hex(body))
	host := server.Listener.Addr().String()
	apply := func(mode RoutingMode) bool {
		t.Helper()
		applied, err := core.ApplyProfileWithOptions(p, time.Now(), ApplyOptions{AllowedRuleSetHosts: []string{host}, RoutingMode: mode})
		if err != nil {
			t.Fatal(err)
		}
		return applied
	}
	if _, err := core.ApplyProfileWithOptions(p, time.Now(), ApplyOptions{RoutingMode: "smart"}); !errors.Is(err, ErrRoutingModeInvalid) {
		t.Fatalf("invalid mode: %v", err)
	}
	if !apply(RoutingModeGlobal) {
		t.Fatal("global apply skipped")
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	checkGlobal := func(when string) {
		t.Helper()
		options, _ := factory.last()
		if options.Route.Final != "selected" || len(options.Route.Rules) != 1 || len(options.Route.RuleSet) != 0 {
			t.Fatalf("%s: global route %+v", when, options.Route)
		}
		if status := core.Status(); status.RoutingMode != RoutingModeGlobal || len(status.RuleSets) != 0 {
			t.Fatalf("%s: status %+v", when, status)
		}
	}
	checkGlobal("apply")
	if downloads.Load() != 0 {
		t.Fatalf("dropped rule set downloaded %d times", downloads.Load())
	}
	if apply(RoutingModeGlobal) {
		t.Fatal("same revision and mode re-applied")
	}
	if err := core.Reload(); err != nil {
		t.Fatal(err)
	}
	checkGlobal("reload")

	if !apply(RoutingModeRules) {
		t.Fatal("switching to rules with the same revision was skipped")
	}
	options, _ := factory.last()
	if options.Route.Final != "direct" || !hasRuleSetRule(options) || downloads.Load() == 0 {
		t.Fatalf("rules route %+v, downloads %d", options.Route, downloads.Load())
	}
	if status := core.Status(); status.RoutingMode != RoutingModeRules || status.Revision != "modes" {
		t.Fatalf("status %+v", status)
	}
}
