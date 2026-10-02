package runtime

import (
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

func systemProxyGet(t *testing.T, status SystemProxyStatus, target string) (int, error) {
	t.Helper()
	proxyURL := &url.URL{Scheme: "http", Host: net.JoinHostPort(status.Listen, strconv.Itoa(int(status.Port)))}
	transport := &http.Transport{Proxy: http.ProxyURL(proxyURL), DisableKeepAlives: true}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 5 * time.Second}
	response, err := client.Get(target)
	if err != nil {
		return 0, err
	}
	response.Body.Close()
	return response.StatusCode, nil
}

func targetPort(t *testing.T, server *httptest.Server) uint16 {
	t.Helper()
	port, err := strconv.Atoi(server.URL[len("http://127.0.0.1:"):])
	if err != nil {
		t.Fatal(err)
	}
	return uint16(port)
}

func listening(status SystemProxyStatus) bool {
	conn, err := net.DialTimeout("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(int(status.Port))), 500*time.Millisecond)
	if err != nil {
		return false
	}
	conn.Close()
	return true
}

// TestSystemProxyFollowsSelectedNodeAndRules runs the standard (non-TUN)
// core with the system proxy toggled at runtime: traffic follows the
// selected node and the profile's DIRECT rule, counts toward traffic, and the
// listener is closed after disable.
func TestSystemProxyFollowsSelectedNodeAndRules(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })
	directTarget := httptest.NewServer(handler)
	defer directTarget.Close()
	proxiedTarget := httptest.NewServer(handler)
	defer proxiedTarget.Close()
	serverPort := startShadowsocksServer(t)
	deadPort := freePort(t)
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "system-1", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{
			{ID: "alive", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "a0", 0, serverPort)}},
			{ID: "dead", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "d0", 0, deadPort)}},
		},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "alive"},
		Routing: profile.Routing{
			Rules: []profile.RoutingRule{{ID: "direct-port", Match: profile.RoutingMatch{Ports: []uint16{targetPort(t, directTarget)}}, Action: profile.RoutingAction{Type: "direct"}}},
			Final: profile.RoutingAction{Type: "proxy", Target: "selected"},
		},
	}
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if status := core.Status().SystemProxy; !status.Available || status.Enabled || status.Listening {
		t.Fatalf("initial status: %#v", status)
	}
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	// Selecting while stopped must survive Start: the built selector default
	// still says "alive".
	if err := core.SelectNode("dead"); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	for _, inbound := range core.built.Options.Inbounds {
		if inbound.Tag == config.SystemProxyInboundTag {
			t.Fatal("system proxy rendered while disabled")
		}
	}

	status, err := core.SetSystemProxy(true)
	if err != nil {
		t.Fatal(err)
	}
	if !status.Enabled || !status.Listening || status.Listen != "127.0.0.1" || status.Port == 0 || status.Port == core.LocalProxyEndpoints()[0].Port {
		t.Fatalf("enabled status: %#v", status)
	}
	if again, err := core.SetSystemProxy(true); err != nil || again.Port != status.Port || !again.Listening {
		t.Fatalf("enable is not idempotent: %#v %v", again, err)
	}
	if core.Status().SystemProxy.Port != status.Port {
		t.Fatalf("status: %#v", core.Status().SystemProxy)
	}
	// Other local proxy connections keep working while toggling.
	local := core.LocalProxyEndpoints()[0]
	tunnel, connectStatus, err := httpConnect(local, local.Username, local.Password, proxiedTarget.Listener.Addr().String())
	if err != nil || connectStatus[:3] != "200" {
		t.Fatalf("local proxy CONNECT: %q %v", connectStatus, err)
	}
	defer tunnel.Close()

	// Selected node is dead: the DIRECT rule still works, proxied traffic fails.
	before := core.Traffic()
	if code, err := systemProxyGet(t, status, directTarget.URL); err != nil || code != http.StatusNoContent {
		t.Fatalf("direct via system proxy: %d %v", code, err)
	}
	if code, err := systemProxyGet(t, status, proxiedTarget.URL); err == nil && code == http.StatusNoContent {
		t.Fatal("proxied request succeeded through the dead selected node")
	}
	// select-node on the standard core switches new system proxy flows.
	if err = core.SelectNode("alive"); err != nil {
		t.Fatal(err)
	}
	if code, err := systemProxyGet(t, status, proxiedTarget.URL); err != nil || code != http.StatusNoContent {
		t.Fatalf("proxied via selected node: %d %v", code, err)
	}
	waitTrafficCounted(t, core, before)
	if code, err := tunnelGet(tunnel, proxiedTarget.Listener.Addr().String()); err != nil || code != http.StatusNoContent {
		t.Fatalf("local proxy tunnel interrupted by toggle: %d %v", code, err)
	}

	// Hot reload keeps the listener and its port.
	next := *p
	next.Revision = "system-2"
	if _, err = core.ApplyProfile(&next, time.Now()); err != nil {
		t.Fatal(err)
	}
	if got := core.SystemProxyStatus(); got.Port != status.Port || !got.Listening || !listening(got) {
		t.Fatalf("after reload: %#v", got)
	}
	if code, err := systemProxyGet(t, status, proxiedTarget.URL); err != nil || code != http.StatusNoContent {
		t.Fatalf("after reload: %d %v", code, err)
	}

	disabled, err := core.SetSystemProxy(false)
	if err != nil {
		t.Fatal(err)
	}
	if disabled.Enabled || disabled.Listening || disabled.Port != 0 {
		t.Fatalf("disabled status: %#v", disabled)
	}
	if listening(status) {
		t.Fatal("system proxy still accepts connections after disable")
	}
	for _, inbound := range core.built.Options.Inbounds {
		if inbound.Tag == config.SystemProxyInboundTag {
			t.Fatal("disabled system proxy would come back on restart")
		}
	}

	// Enabled while stopped: listens only once started, on the persisted port.
	if err = core.Stop(); err != nil {
		t.Fatal(err)
	}
	stopped, err := core.SetSystemProxy(true)
	if err != nil || !stopped.Enabled || stopped.Listening || stopped.Port != status.Port {
		t.Fatalf("enable while stopped: %#v %v", stopped, err)
	}
	if listening(stopped) {
		t.Fatal("listening while core is stopped")
	}
	if err = core.Start(); err != nil {
		t.Fatal(err)
	}
	if got := core.SystemProxyStatus(); !got.Listening || !listening(got) {
		t.Fatalf("not listening after start: %#v", got)
	}
	if code, err := systemProxyGet(t, core.SystemProxyStatus(), proxiedTarget.URL); err != nil || code != http.StatusNoContent {
		t.Fatalf("after restart, selection lost: %d %v", code, err)
	}
}

func TestSystemProxyStartFallsBackWhenPortTaken(t *testing.T) {
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	p := testProfile("r1", "edge.example.com", "8.8.8.8")
	p.Nodes[0].Ingresses[0].Credentials.Shadowsocks.UserKey = testSSKey
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	first, err := core.SetSystemProxy(true)
	if err != nil {
		t.Fatal(err)
	}
	// Someone takes the port while the core is stopped.
	squatter, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(int(first.Port))))
	if err != nil {
		t.Fatal(err)
	}
	defer squatter.Close()
	if err = core.Start(); err != nil {
		t.Fatalf("start with taken system proxy port: %v", err)
	}
	defer core.Stop()
	got := core.SystemProxyStatus()
	if got.Port == first.Port || !got.Listening || !listening(got) {
		t.Fatalf("no fallback: %#v -> %#v", first, got)
	}
}

func TestSystemProxyUnavailableInTUNCore(t *testing.T) {
	factory := &fakeFactory{}
	c := newCore(profile.PlatformCapabilities{Platform: "macos", TUN: profile.TUNCapabilities{Enabled: true}}, factory.create)
	if _, err := c.SetSystemProxy(true); !errors.Is(err, ErrSystemProxyUnavailable) {
		t.Fatalf("TUN core: %v", err)
	}
	if c.Status().SystemProxy.Available {
		t.Fatal("TUN core reports system proxy available")
	}
}
