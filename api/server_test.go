package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	coreruntime "github.com/peakpassvpn/ppvpn-core/internal/runtime"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

const testSecret = "0123456789abcdef0123456789abcdef"

func apiProfile() *profile.Profile {
	label := "Tokyo A"
	n := profile.Node{ID: "node", Name: "Tokyo", EntryKey: "cn-optimized", EntryLabel: "CN Optimized", Exit: profile.Exit{Region: "Tokyo"}, Capabilities: profile.Capabilities{TCP: true, UDP: true}, Ingresses: []profile.Ingress{{Role: profile.IngressRolePrimary, EndpointKey: "9001", Label: &label, Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: "edge.example.com", IP: "8.8.8.8", Port: 443}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}}, Capabilities: profile.Capabilities{TCP: true, UDP: true}}}}
	return &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "r1", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{n}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
}
func testServer(t *testing.T) (*Server, *coreruntime.Core) {
	t.Helper()
	core := coreruntime.New(profile.PlatformCapabilities{})
	server, err := NewServer(core, testSecret)
	if err != nil {
		t.Fatal(err)
	}
	return server, core
}
func request(t *testing.T, server *Server, path string, body any, authenticated bool) *httptest.ResponseRecorder {
	t.Helper()
	var data []byte
	if body != nil {
		data, _ = json.Marshal(body)
	}
	req := httptest.NewRequest(http.MethodPost, path, bytes.NewReader(data))
	if authenticated {
		req.Header.Set("Authorization", "Bearer "+testSecret)
		req.Header.Set("X-Core-API-Version", "1")
	}
	rec := httptest.NewRecorder()
	server.Handler().ServeHTTP(rec, req)
	return rec
}
func TestUnauthenticatedRejected(t *testing.T) {
	server, _ := testServer(t)
	rec := request(t, server, "/v1/get-version", nil, false)
	if rec.Code != http.StatusUnauthorized || !strings.Contains(rec.Body.String(), "UNAUTHENTICATED") {
		t.Fatal(rec.Body.String())
	}
}
func TestAPIVersionMismatchRejected(t *testing.T) {
	server, _ := testServer(t)
	req := httptest.NewRequest(http.MethodPost, "/v1/get-version", nil)
	req.Header.Set("Authorization", "Bearer "+testSecret)
	req.Header.Set("X-Core-API-Version", "99")
	rec := httptest.NewRecorder()
	server.Handler().ServeHTTP(rec, req)
	if !strings.Contains(rec.Body.String(), "CORE_API_UNSUPPORTED") {
		t.Fatal(rec.Body.String())
	}
}
func TestApplyAndListNeverExposeCredentials(t *testing.T) {
	server, _ := testServer(t)
	rec := request(t, server, "/v1/apply-profile", map[string]any{"profile": apiProfile()}, true)
	if rec.Code != http.StatusOK {
		t.Fatal(rec.Body.String())
	}
	listed := request(t, server, "/v1/list-nodes", nil, true)
	body := listed.Body.String()
	if strings.Contains(body, "AAAAAAAAAAAAAAAAAAAAAA==") || strings.Contains(body, "credentials") || !strings.Contains(body, "Tokyo") {
		t.Fatal(body)
	}
}
func TestValidationErrorIsStructuredAndRedacted(t *testing.T) {
	server, _ := testServer(t)
	p := apiProfile()
	p.Nodes[0].Ingresses[0].Endpoint.IP = "198.18.0.1"
	rec := request(t, server, "/v1/validate-profile", map[string]any{"profile": p}, true)
	body := rec.Body.String()
	if !strings.Contains(body, "ENTRY_IP_NOT_PUBLIC") || !strings.Contains(body, "nodes[0].ingresses[0].endpoint.ip") || strings.Contains(body, "AAAAAAAAAAAAAAAAAAAAAA==") {
		t.Fatal(body)
	}
}

func TestUnknownMethodUsesEnvelope(t *testing.T) {
	server, _ := testServer(t)
	rec := request(t, server, "/v1/not-real", nil, true)
	if rec.Code != http.StatusNotFound || !strings.Contains(rec.Body.String(), "API_NOT_FOUND") {
		t.Fatal(rec.Body.String())
	}
}

func TestSystemProxyUnavailableWithoutStateOrInTUNCore(t *testing.T) {
	server, _ := testServer(t)
	for _, path := range []string{"/v1/get-system-proxy-endpoints", "/v1/set-system-proxy"} {
		rec := request(t, server, path, map[string]any{"enabled": true}, true)
		if path == "/v1/get-system-proxy-endpoints" {
			rec = request(t, server, path, map[string]any{}, true)
		}
		if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), "SYSTEM_PROXY_UNAVAILABLE") {
			t.Fatalf("%s: %s", path, rec.Body.String())
		}
	}
	tun := coreruntime.NewWithLocalProxyState(profile.PlatformCapabilities{TUN: profile.TUNCapabilities{Enabled: true}}, filepath.Join(t.TempDir(), "local-proxies.json"))
	tunServer, err := NewServer(tun, testSecret)
	if err != nil {
		t.Fatal(err)
	}
	if rec := request(t, tunServer, "/v1/set-system-proxy", map[string]any{"enabled": true}, true); !strings.Contains(rec.Body.String(), "SYSTEM_PROXY_UNAVAILABLE") {
		t.Fatal(rec.Body.String())
	}
}

func TestSetSystemProxyToggleAndStatus(t *testing.T) {
	core := coreruntime.NewWithLocalProxyState(profile.PlatformCapabilities{}, filepath.Join(t.TempDir(), "local-proxies.json"))
	server, err := NewServer(core, testSecret)
	if err != nil {
		t.Fatal(err)
	}
	status := request(t, server, "/v1/get-status", map[string]any{}, true)
	if !strings.Contains(status.Body.String(), `"system_proxy":{"available":true,"enabled":false,"listening":false}`) {
		t.Fatal(status.Body.String())
	}
	if rec := request(t, server, "/v1/set-system-proxy", map[string]any{}, true); !strings.Contains(rec.Body.String(), "REQUEST_INVALID") {
		t.Fatal(rec.Body.String())
	}
	enabled := request(t, server, "/v1/set-system-proxy", map[string]any{"enabled": true}, true)
	body := enabled.Body.String()
	if enabled.Code != http.StatusOK || !strings.Contains(body, `"enabled":true`) || !strings.Contains(body, `"listening":false`) ||
		!strings.Contains(body, `"listen":"127.0.0.1"`) || !strings.Contains(body, `"protocols":["http","socks5"]`) {
		t.Fatal(body)
	}
	if rec := request(t, server, "/v1/get-system-proxy-endpoints", map[string]any{}, true); !strings.Contains(rec.Body.String(), `"enabled":true`) {
		t.Fatal(rec.Body.String())
	}
	disabled := request(t, server, "/v1/set-system-proxy", map[string]any{"enabled": false}, true)
	if disabled.Code != http.StatusOK || !strings.Contains(disabled.Body.String(), `"enabled":false`) || strings.Contains(disabled.Body.String(), `"port"`) {
		t.Fatal(disabled.Body.String())
	}
}

func TestLocalProxyMetadataAndCredentialAreSeparated(t *testing.T) {
	capabilities := profile.PlatformCapabilities{
		LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"},
	}
	core := coreruntime.NewWithLocalProxyState(capabilities, filepath.Join(t.TempDir(), "local-proxies.json"))
	server, err := NewServer(core, testSecret)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = core.ApplyProfile(apiProfile(), time.Now()); err != nil {
		t.Fatal(err)
	}

	metadata := request(t, server, "/v1/get-local-proxy-metadata", map[string]any{}, true)
	if metadata.Code != http.StatusOK ||
		!strings.Contains(metadata.Body.String(), `"protocols":["http","socks5"]`) ||
		!strings.Contains(metadata.Body.String(), `"auth_required":true`) ||
		strings.Contains(metadata.Body.String(), "username") ||
		strings.Contains(metadata.Body.String(), "password") {
		t.Fatal(metadata.Body.String())
	}

	credential := request(t, server, "/v1/get-local-proxy-credential", map[string]any{"node_id": "node"}, true)
	if credential.Code != http.StatusOK ||
		!strings.Contains(credential.Body.String(), `"username"`) ||
		!strings.Contains(credential.Body.String(), `"password"`) {
		t.Fatal(credential.Body.String())
	}
	missing := request(t, server, "/v1/get-local-proxy-credential", map[string]any{"node_id": "missing"}, true)
	if missing.Code != http.StatusBadRequest ||
		!strings.Contains(missing.Body.String(), `"code":"NODE_NOT_FOUND"`) {
		t.Fatal(missing.Body.String())
	}
	// 0.5.12: every metadata entry has a kind; the routed user is last.
	body := metadata.Body.String()
	if !strings.Contains(body, `{"kind":"node","node_id":"node",`) || !strings.HasSuffix(strings.TrimSpace(body[:strings.LastIndex(body, "]")]), `"auth_required":true}`) ||
		strings.Index(body, `"kind":"routed","node_id":""`) < strings.Index(body, `"kind":"node"`) {
		t.Fatal(body)
	}
	if !strings.Contains(credential.Body.String(), `"kind":"node"`) {
		t.Fatal(credential.Body.String())
	}
	routed := request(t, server, "/v1/get-local-proxy-credential", map[string]any{"kind": "routed"}, true)
	if routed.Code != http.StatusOK || !strings.Contains(routed.Body.String(), `"kind":"routed","node_id":""`) ||
		!strings.Contains(routed.Body.String(), `"password"`) || strings.Contains(routed.Body.String(), `-node"`) {
		t.Fatal(routed.Body.String())
	}
	for name, c := range map[string]struct {
		body  map[string]any
		code  string
		field string
	}{
		"routed with node_id": {map[string]any{"kind": "routed", "node_id": "node"}, "REQUEST_INVALID", "node_id"},
		"unknown kind":        {map[string]any{"kind": "system"}, "REQUEST_INVALID", "kind"},
		"empty node_id":       {map[string]any{"node_id": ""}, "NODE_NOT_FOUND", "node_id"}, // not an implicit routed
	} {
		rec := request(t, server, "/v1/get-local-proxy-credential", c.body, true)
		if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), `"code":"`+c.code+`"`) || !strings.Contains(rec.Body.String(), `"field":"`+c.field+`"`) {
			t.Fatalf("%s: %s", name, rec.Body.String())
		}
	}
}

func TestProbeEntrancesMethodAndShape(t *testing.T) {
	server, core := testServer(t)
	if _, err := core.ApplyProfile(apiProfile(), time.Now()); err != nil {
		t.Fatal(err)
	}
	bad := request(t, server, "/v1/probe-entrances", map[string]any{"method": "udp"}, true)
	if bad.Code != http.StatusBadRequest || !strings.Contains(bad.Body.String(), `"code":"PROBE_METHOD_UNSUPPORTED"`) {
		t.Fatal(bad.Body.String())
	}
	for _, method := range []string{"tcp", "icmp"} {
		rec := request(t, server, "/v1/probe-entrances", map[string]any{"method": method, "timeout_ms": 50, "node_ids": []string{"node"}}, true)
		body := rec.Body.String()
		if rec.Code != http.StatusOK || !strings.Contains(body, `"method":"`+method+`"`) || !strings.Contains(body, `"endpoint_key":"9001","ingress_role":"primary"`) ||
			!strings.Contains(body, `"ingresses":[{"endpoint_key":"9001","label":"Tokyo A","replica_ordinal":0,"role":"primary"`) || !strings.Contains(body, `"latency_ms"`) || strings.Contains(body, "connect_ms") {
			t.Fatal(body)
		}
	}
	missing := request(t, server, "/v1/probe-entrances", map[string]any{"node_ids": []string{"missing"}}, true)
	if !strings.Contains(missing.Body.String(), `"code":"NODE_NOT_FOUND"`) {
		t.Fatal(missing.Body.String())
	}
}

func TestLocalProxyAPIsReportDisabledCore(t *testing.T) {
	server, core := testServer(t) // local proxy disabled, as with `serve --tun --local-proxy=false`
	if _, err := core.ApplyProfile(apiProfile(), time.Now()); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{"/v1/get-local-proxy-metadata", "/v1/get-local-proxy-endpoints", "/v1/get-local-proxy-credential", "/v1/probe-availability"} {
		rec := request(t, server, path, map[string]any{"node_id": "node", "target": "http://example.com"}, true)
		if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), `"code":"LOCAL_PROXY_DISABLED"`) {
			t.Fatalf("%s: %s", path, rec.Body.String())
		}
	}
	nodes := request(t, server, "/v1/list-nodes", map[string]any{}, true)
	if !strings.Contains(nodes.Body.String(), `"protocol":"shadowsocks"`) || !strings.Contains(nodes.Body.String(), `"ingresses":[{"endpoint_key":"9001","label":"Tokyo A","replica_ordinal":0,"role":"primary","protocol":"shadowsocks"}]`) ||
		!strings.Contains(nodes.Body.String(), `"entry_key":"cn-optimized","entry_label":"CN Optimized"`) || strings.Contains(nodes.Body.String(), "country_code") {
		t.Fatal(nodes.Body.String())
	}
}

func TestFoldedErrorIsLoggedWithStageButNotReturned(t *testing.T) {
	dir := t.TempDir()
	statePath := filepath.Join(dir, "local-proxies.json")
	if err := os.WriteFile(statePath, []byte(`{"version":2,"prefix":"abcde","password":"secret","port":7890}`), 0o644); err != nil {
		t.Fatal(err)
	}
	core := coreruntime.NewWithLocalProxyState(profile.PlatformCapabilities{LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}}, statePath)
	server, err := NewServer(core, testSecret)
	if err != nil {
		t.Fatal(err)
	}
	var logged bytes.Buffer
	server.SetLogger(corelog.New(&logged))
	rec := request(t, server, "/v1/apply-profile", map[string]any{"profile": apiProfile()}, true)
	body := rec.Body.String()
	if !strings.Contains(body, `"code":"CORE_OPERATION_FAILED"`) || strings.Contains(body, "private") || strings.Contains(body, "local-proxies") {
		t.Fatalf("response not folded: %s", body)
	}
	line := logged.String()
	for _, want := range []string{"level=error", "msg=CORE_OPERATION_FAILED", "path=/v1/apply-profile", "stage=apply/local-proxy-state", "not private", "*runtime.StageError > *fmt.wrapError"} {
		if !strings.Contains(line, want) {
			t.Errorf("log missing %q: %s", want, line)
		}
	}
	if strings.Contains(line, "secret") {
		t.Fatalf("log leaked state secret: %s", line)
	}
}

func TestRuleSetHostsArePinnedAtValidateAndApply(t *testing.T) {
	server, core := testServer(t)
	p := apiProfile()
	p.Routing.RuleSets = []profile.RuleSet{{ID: "cn-ip", URL: "https://api.example.com/api/v1/proxy-profile/rule-sets/cn-ip.srs", SHA256: strings.Repeat("0", 64)}}
	p.Routing.Rules = []profile.RoutingRule{{ID: "geoip-cn", Match: profile.RoutingMatch{RuleSetIDs: []string{"cn-ip"}}, Action: profile.RoutingAction{Type: "direct"}}}
	for _, path := range []string{"/v1/validate-profile", "/v1/apply-profile"} {
		rec := request(t, server, path, map[string]any{"profile": p, "allowed_rule_set_hosts": []string{"evil.example"}}, true)
		if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), "RULE_SET_HOST_NOT_ALLOWED") {
			t.Fatalf("%s: %s", path, rec.Body.String())
		}
	}
	rec := request(t, server, "/v1/validate-profile", map[string]any{"profile": p, "allowed_rule_set_hosts": []string{"api.example.com"}}, true)
	if rec.Code != http.StatusOK {
		t.Fatal(rec.Body.String())
	}
	// No state directory: the profile applies and the set is reported.
	rec = request(t, server, "/v1/apply-profile", map[string]any{"profile": p, "allowed_rule_set_hosts": []string{"api.example.com"}}, true)
	if rec.Code != http.StatusOK {
		t.Fatal(rec.Body.String())
	}
	if status := core.Status(); len(status.RuleSets) != 1 || status.RuleSets[0].State != "unavailable" || status.RuleSets[0].Error != "RULE_SET_STORAGE_UNAVAILABLE" {
		t.Fatalf("status: %+v", status.RuleSets)
	}
	rec = request(t, server, "/v1/get-status", nil, true)
	if !strings.Contains(rec.Body.String(), `"rule_sets":[{"id":"cn-ip","state":"unavailable","error":"RULE_SET_STORAGE_UNAVAILABLE"}]`) {
		t.Fatal(rec.Body.String())
	}
}

// routing_mode is optional on validate/apply-profile, strictly checked, and
// reported by get-status; switching it re-applies the same revision.
func TestRoutingModeOnApplyAndStatus(t *testing.T) {
	server, _ := testServer(t)
	p := apiProfile()
	for _, path := range []string{"/v1/validate-profile", "/v1/apply-profile"} {
		rec := request(t, server, path, map[string]any{"profile": p, "routing_mode": "smart"}, true)
		if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), `"code":"ROUTING_MODE_INVALID"`) || !strings.Contains(rec.Body.String(), `"field":"routing_mode"`) {
			t.Fatalf("%s: %d %s", path, rec.Code, rec.Body.String())
		}
	}
	if rec := request(t, server, "/v1/validate-profile", map[string]any{"profile": p, "routing_mode": "global"}, true); rec.Code != http.StatusOK {
		t.Fatal(rec.Body.String())
	}
	for _, step := range []struct {
		mode    any
		applied bool
		status  string
	}{
		{nil, true, `"routing_mode":"rules"`},
		{"global", true, `"routing_mode":"global"`},
		{"global", false, `"routing_mode":"global"`},
		{"rules", true, `"routing_mode":"rules"`},
	} {
		body := map[string]any{"profile": p}
		if step.mode != nil {
			body["routing_mode"] = step.mode
		}
		rec := request(t, server, "/v1/apply-profile", body, true)
		if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), fmt.Sprintf(`"applied":%v`, step.applied)) {
			t.Fatalf("mode %v: %d %s", step.mode, rec.Code, rec.Body.String())
		}
		if rec = request(t, server, "/v1/get-status", nil, true); !strings.Contains(rec.Body.String(), step.status) {
			t.Fatalf("mode %v: status %s", step.mode, rec.Body.String())
		}
	}
}

func TestPinIngressEndpoint(t *testing.T) {
	server, core := testServer(t)
	if rec := request(t, server, "/v1/pin-ingress", map[string]any{"node_id": "node", "endpoint_key": "9001"}, true); rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), "PROFILE_NOT_APPLIED") {
		t.Fatalf("before apply: %d %s", rec.Code, rec.Body.String())
	}
	p := apiProfile()
	if rec := request(t, server, "/v1/apply-profile", map[string]any{"profile": p}, true); rec.Code != http.StatusOK {
		t.Fatal(rec.Body.String())
	}
	node, key := p.Nodes[0].ID, p.Nodes[0].Ingresses[0].EndpointKey
	for _, step := range []struct {
		body map[string]any
		code int
		want string
	}{
		{map[string]any{"node_id": "missing", "endpoint_key": key}, http.StatusBadRequest, `"code":"NODE_NOT_FOUND"`},
		{map[string]any{"node_id": node, "endpoint_key": "missing"}, http.StatusBadRequest, `"code":"INGRESS_NOT_FOUND"`},
		{map[string]any{"node_id": node, "endpoint_key": ""}, http.StatusBadRequest, `"code":"INGRESS_NOT_FOUND"`},
		{map[string]any{"node_id": node, "endpoint_key": key}, http.StatusOK, `"endpoint_key":"` + key + `"`},
	} {
		rec := request(t, server, "/v1/pin-ingress", step.body, true)
		if rec.Code != step.code || !strings.Contains(rec.Body.String(), step.want) {
			t.Fatalf("%v: %d %s", step.body, rec.Code, rec.Body.String())
		}
	}
	if rec := request(t, server, "/v1/get-status", nil, true); !strings.Contains(rec.Body.String(), `"pinned_endpoint_key":"`+key+`"`) {
		t.Fatalf("status: %s", rec.Body.String())
	}
	if rec := request(t, server, "/v1/pin-ingress", map[string]any{"node_id": node, "endpoint_key": nil}, true); rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"endpoint_key":null`) {
		t.Fatalf("unpin: %d %s", rec.Code, rec.Body.String())
	}
	if status := core.Status(); len(status.Nodes) != 1 || status.Nodes[0].PinnedEndpointKey != nil {
		t.Fatalf("status after unpin: %+v", status.Nodes)
	}
}

// GET /v1/debug/goroutines answers only at debug level and only to an
// authenticated caller.
func TestDebugGoroutinesOnlyAtDebugLevel(t *testing.T) {
	server, _ := testServer(t)
	get := func(authenticated bool) *httptest.ResponseRecorder {
		req := httptest.NewRequest(http.MethodGet, "/v1/debug/goroutines", nil)
		if authenticated {
			req.Header.Set("Authorization", "Bearer "+testSecret)
		}
		rec := httptest.NewRecorder()
		server.Handler().ServeHTTP(rec, req)
		return rec
	}
	if rec := get(true); rec.Code != http.StatusNotFound || !strings.Contains(rec.Body.String(), "API_NOT_FOUND") {
		t.Fatalf("info level: %d %s", rec.Code, rec.Body.String())
	}
	log := corelog.New(io.Discard)
	_ = log.SetLevel(corelog.LevelDebug)
	server.SetLogger(log)
	if rec := get(false); rec.Code != http.StatusUnauthorized {
		t.Fatalf("unauthenticated: %d", rec.Code)
	}
	rec := get(true)
	if rec.Code != http.StatusOK || !strings.HasPrefix(rec.Body.String(), "goroutine ") || !strings.Contains(rec.Body.String(), "debugGoroutines") {
		t.Fatalf("debug level: %d %.200s", rec.Code, rec.Body.String())
	}
}

// Offline probes fail fast as NO_DEFAULT_INTERFACE, retryable.
func TestNoDefaultInterfaceIsRetryable(t *testing.T) {
	var structured *apiErr
	if !errors.As(coreError(coreruntime.ErrNoDefaultInterface), &structured) || structured.Detail.Code != "NO_DEFAULT_INTERFACE" || !structured.Detail.Retryable {
		t.Fatalf("mapped to %#v", structured)
	}
}
