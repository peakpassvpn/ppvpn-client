package runtime

import (
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"strconv"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/protocol/socks"
)

// socks5Dial opens a SOCKS5 CONNECT to an IP target through listen:port,
// authenticating when username is set (the system proxy has no users).
func socks5Dial(t *testing.T, listen string, port uint16, username, password, target string) net.Conn {
	t.Helper()
	client := socks.NewClient(N.SystemDialer, M.ParseSocksaddrHostPort(listen, port), socks.Version5, username, password)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	conn, err := client.DialContext(ctx, N.NetworkTCP, M.ParseSocksaddr(target))
	if err != nil {
		t.Fatalf("socks5 %s via %d: %v", target, port, err)
	}
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	return conn
}

// TestRoutedLocalProxyUserFollowsProfileRules: the bare prefix on the shared
// local proxy routes like the system proxy (7891): profile rules first
// (DIRECT included), then the selected node; select-node and routing_mode
// apply to new connections; node usernames stay pinned; bad passwords are
// rejected; traffic is counted.
func TestRoutedLocalProxyUserFollowsProfileRules(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })
	directTarget := httptest.NewServer(handler)
	defer directTarget.Close()
	proxiedTarget := httptest.NewServer(handler)
	defer proxiedTarget.Close()
	proxiedPort := strconv.Itoa(int(targetPort(t, proxiedTarget)))
	nodeA, nodeB := &destinationRecorder{}, &destinationRecorder{}
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "routed-1", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{
			{ID: "a", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "a0", 0, startShadowsocksServerWithTracker(t, nodeA))}},
			{ID: "b", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "b0", 0, startShadowsocksServerWithTracker(t, nodeB))}},
		},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "a"},
		Routing: profile.Routing{
			Rules: []profile.RoutingRule{
				{ID: "direct-domain", Match: profile.RoutingMatch{Domains: []string{"localhost"}}, Action: profile.RoutingAction{Type: "direct"}},
				{ID: "direct-port", Match: profile.RoutingMatch{Ports: []uint16{targetPort(t, directTarget)}}, Action: profile.RoutingAction{Type: "direct"}},
			},
			Final: profile.RoutingAction{Type: "proxy", Target: "selected"},
		},
	}
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	routed, err := core.LocalProxyRoutedCredential()
	if err != nil {
		t.Fatal(err)
	}
	endpoint := localproxy.Endpoint{Listen: routed.Listen, Port: routed.Port}
	nodeUser := core.LocalProxyEndpoints()[0] // node "a"
	reset := func() { nodeA.reset(); nodeB.reset() }
	// connectGet runs a GET through an authenticated CONNECT tunnel.
	connectGet := func(username, password, target string) {
		t.Helper()
		tunnel, status, err := httpConnect(endpoint, username, password, target)
		if err != nil || status[:3] != "200" {
			t.Fatalf("CONNECT %s as %s: %q %v", target, username, status, err)
		}
		defer tunnel.Close()
		if code, err := tunnelGet(tunnel, target); err != nil || code != http.StatusNoContent {
			t.Fatalf("GET %s as %s: %d %v", target, username, code, err)
		}
	}
	socksGet := func(port uint16, username, password, target string) {
		t.Helper()
		conn := socks5Dial(t, routed.Listen, port, username, password, target)
		defer conn.Close()
		if code, err := tunnelGet(conn, target); err != nil || code != http.StatusNoContent {
			t.Fatalf("SOCKS5 GET %s via %d: %d %v", target, port, code, err)
		}
	}
	// direct: neither node saw the target.
	direct := func(what, destination string) {
		t.Helper()
		if nodeA.has(destination) || nodeB.has(destination) {
			t.Fatalf("%s went through a node, want DIRECT", what)
		}
	}

	// 1. HTTP CONNECT to a domain matching a DIRECT rule goes direct.
	reset()
	connectGet(routed.Username, routed.Password, "localhost:"+proxiedPort)
	direct("CONNECT localhost (domain rule)", "localhost:"+proxiedPort)

	// 2. SOCKS5 to an IP matching a DIRECT (port) rule goes direct.
	directIP := directTarget.Listener.Addr().String()
	socksGet(routed.Port, routed.Username, routed.Password, directIP)
	direct("SOCKS5 to the direct port", directIP)

	// 3. Anything else goes to the selected node "a", and is counted.
	proxiedIP := proxiedTarget.Listener.Addr().String()
	before := core.Traffic()
	socksGet(routed.Port, routed.Username, routed.Password, proxiedIP)
	nodeA.wait(t, proxiedIP)
	if after := core.Traffic(); after.UploadBytes <= before.UploadBytes || after.DownloadBytes <= before.DownloadBytes {
		t.Fatalf("routed traffic not counted: %#v -> %#v", before, after)
	}

	// 4. SOCKS5 to an IP gets no domain (no sniffing outside TUN), so a domain
	// rule cannot match: same result as the system proxy (selected node).
	systemProxy, err := core.SetSystemProxy(true)
	if err != nil {
		t.Fatal(err)
	}
	reset()
	socksGet(systemProxy.Port, "", "", proxiedIP)
	nodeA.wait(t, proxiedIP)
	reset()
	socksGet(routed.Port, routed.Username, routed.Password, proxiedIP)
	nodeA.wait(t, proxiedIP)

	// 5. select-node switches new routed connections; node usernames stay
	// pinned to their node.
	if err = core.SelectNode("b"); err != nil {
		t.Fatal(err)
	}
	reset()
	connectGet(routed.Username, routed.Password, proxiedIP)
	nodeB.wait(t, proxiedIP)
	if nodeA.has(proxiedIP) {
		t.Fatal("routed user still on node a after select-node b")
	}
	reset()
	connectGet(nodeUser.Username, nodeUser.Password, proxiedIP)
	nodeA.wait(t, proxiedIP)

	// 6. Global mode drops the non-baseline rules: the DIRECT domain now goes
	// to the selected node.
	if _, err = core.ApplyProfileWithOptions(p, time.Now(), ApplyOptions{RoutingMode: RoutingModeGlobal}); err != nil {
		t.Fatal(err)
	}
	reset()
	connectGet(routed.Username, routed.Password, "localhost:"+proxiedPort)
	nodeB.wait(t, "localhost:"+proxiedPort)

	// 7. A wrong password is rejected: 407 on HTTP, RFC 1929 failure on SOCKS5.
	if tunnel, status, err := httpConnect(endpoint, routed.Username, "wrong", proxiedIP); err != nil || status[:3] != "407" {
		t.Fatalf("wrong password: %q %v", status, err)
	} else {
		tunnel.Close()
	}
	if conn, status, err := socks5Connect(endpoint, routed.Username, "wrong", proxiedIP); err != nil || status == 0 {
		if conn != nil {
			conn.Close()
		}
		t.Fatalf("SOCKS5 wrong password: status %d %v", status, err)
	}
}
