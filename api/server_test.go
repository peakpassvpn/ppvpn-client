package api

import (
	"bytes"
	"encoding/json"
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
	n := profile.Node{ID: "node", Name: "Tokyo", EntryKey: "cn-optimized", EntryLabel: "CN Optimized", Exit: profile.Exit{Region: "Tokyo"}, Capabilities: profile.Capabilities{TCP: true, UDP: true}, Ingresses: []profile.Ingress{{Role: profile.IngressRolePrimary, EndpointKey: "9001", Label: &label, Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: "edge.example.com", IP: "8.8.8.8", Port: 443}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}}, Capabilities: profile.Capabilities{TCP: true, UDP: true}}}}
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
