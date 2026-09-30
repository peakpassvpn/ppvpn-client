package runtime

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"errors"
	"net/http"
	"net/http/httptest"
	"slices"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/rulesets"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/common/srs"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

func domainRuleSet(t *testing.T, suffixes ...string) []byte {
	t.Helper()
	var buffer bytes.Buffer
	rule := option.HeadlessRule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultHeadlessRule{DomainSuffix: badoption.Listable[string](suffixes)}}
	if err := srs.Write(&buffer, option.PlainRuleSet{Rules: []option.HeadlessRule{rule}}, C.RuleSetVersion3); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

func sha256Hex(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

// optionsFactory is a fake engine factory that keeps every built config.
type optionsFactory struct {
	mu      sync.Mutex
	options []option.Options
}

func (f *optionsFactory) create(_ context.Context, options option.Options) (engine, error) {
	f.mu.Lock()
	f.options = append(f.options, options)
	f.mu.Unlock()
	return &fakeEngine{}, nil
}

func (f *optionsFactory) last() (option.Options, int) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.options) == 0 {
		return option.Options{}, 0
	}
	return f.options[len(f.options)-1], len(f.options)
}

func ruleSetTestProfile(url, sha string) *profile.Profile {
	p := testProfile("rule-sets", "a.example", "8.8.8.8")
	p.Routing.RuleSets = []profile.RuleSet{{ID: "cn-site", URL: url, SHA256: sha, UpdateIntervalSeconds: 3600}}
	p.Routing.Rules = []profile.RoutingRule{{ID: "cn", Match: profile.RoutingMatch{RuleSetIDs: []string{"cn-site"}}, Action: profile.RoutingAction{Type: "direct"}}}
	return p
}

func hasRuleSetRule(options option.Options) bool {
	if options.Route == nil {
		return false
	}
	for _, rule := range options.Route.Rules {
		if slices.Contains(rule.DefaultOptions.RuleSet, config.RuleSetTag("cn-site")) {
			return len(options.Route.RuleSet) == 1
		}
	}
	return false
}

// A rule set that cannot be downloaded never blocks apply or start: its rule
// is skipped and reported; once the set arrives the core rebuilds with it.
func TestRuleSetUnavailableThenRecovered(t *testing.T) {
	body := domainRuleSet(t, "cn.example")
	var failing atomic.Bool
	failing.Store(true)
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		if failing.Load() {
			http.Error(w, "down", http.StatusBadGateway)
			return
		}
		_, _ = w.Write(body)
	}))
	defer server.Close()

	factory := &optionsFactory{}
	core := newCore(profile.PlatformCapabilities{}, factory.create)
	core.enableRuleSets(t.TempDir(), func(o *rulesets.Options) {
		o.TLSConfig = &tls.Config{RootCAs: server.Client().Transport.(*http.Transport).TLSClientConfig.RootCAs}
		o.RetryMin = 20 * time.Millisecond
	})
	defer core.ruleSets.Close()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	events := core.Subscribe(ctx, 64)

	p := ruleSetTestProfile(server.URL+"/cn-site.srs", sha256Hex(body))
	host := server.Listener.Addr().String()
	if _, err := core.ApplyProfileWithOptions(p, time.Now(), ApplyOptions{AllowedRuleSetHosts: []string{"other.example"}}); !isValidationCode(err, "RULE_SET_HOST_NOT_ALLOWED") {
		t.Fatalf("foreign host: %v", err)
	}
	if _, err := core.ApplyProfileWithOptions(p, time.Now(), ApplyOptions{AllowedRuleSetHosts: []string{host}}); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	status := core.Status()
	if len(status.RuleSets) != 1 || status.RuleSets[0].State != rulesets.StateUnavailable || status.RuleSets[0].Error != rulesets.ErrHTTPStatus {
		t.Fatalf("status: %+v", status.RuleSets)
	}
	if options, _ := factory.last(); hasRuleSetRule(options) || len(options.Route.Rules) != 0 {
		t.Fatalf("unavailable rule set rendered: %+v", options.Route)
	}

	failing.Store(false)
	deadline := time.After(5 * time.Second)
	for {
		options, _ := factory.last()
		if hasRuleSetRule(options) && core.Status().RuleSets[0].State == rulesets.StateReady {
			break
		}
		select {
		case <-deadline:
			t.Fatalf("rule set never took effect: %+v", core.Status().RuleSets)
		case <-time.After(10 * time.Millisecond):
		}
	}
	if core.Status().Revision != "rule-sets" {
		t.Fatalf("rebuild changed the revision: %s", core.Status().Revision)
	}
	var states []string
	for len(states) < 2 {
		select {
		case event := <-events:
			if event.Type == EventRuleSetChanged && event.RuleSetID == "cn-site" {
				states = append(states, event.Message)
			}
		case <-deadline:
			t.Fatalf("rule set events: %v", states)
		}
	}
	if !slices.Equal(states, []string{"unavailable", "ready"}) {
		t.Fatalf("rule set events: %v", states)
	}
}

// Without allowed_rule_set_hosts the profile still applies; the set is
// reported unpinned and never fetched.
func TestRuleSetWithoutPinnedHostIsUnavailable(t *testing.T) {
	var requests atomic.Int32
	server := httptest.NewTLSServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { requests.Add(1) }))
	defer server.Close()
	core := newCore(profile.PlatformCapabilities{}, (&optionsFactory{}).create)
	core.EnableRuleSets(t.TempDir())
	defer core.ruleSets.Close()
	p := ruleSetTestProfile(server.URL+"/cn-site.srs", sha256Hex([]byte("x")))
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if status := core.Status().RuleSets; len(status) != 1 || status[0].State != rulesets.StateUnavailable || status[0].Error != rulesets.ErrHostNotPinned {
		t.Fatalf("status: %+v", status)
	}
	if requests.Load() != 0 {
		t.Fatal("unpinned host contacted")
	}
}

func isValidationCode(err error, code string) bool {
	var ve *profile.ValidationError
	return errors.As(err, &ve) && ve.Code == code
}
