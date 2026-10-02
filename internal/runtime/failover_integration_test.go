package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/profile"
	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

const testSSKey = "AAAAAAAAAAAAAAAAAAAAAA=="

// freePort returns a loopback port that is free for both TCP and UDP. Some
// listeners the tests start (the Shadowsocks server, SOCKS inbounds) bind
// both on the same number, and a port picked by TCP alone was occasionally
// already taken for UDP ("listen udp …: address already in use"). The port
// is released before returning, so a listener can still lose a race for it;
// startShadowsocksServerWithTracker retries for that.
func freePort(t *testing.T) uint16 {
	t.Helper()
	for range 50 {
		l, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		port := l.Addr().(*net.TCPAddr).Port
		packet, err := net.ListenPacket("udp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
		l.Close()
		if err == nil {
			packet.Close()
			return uint16(port)
		}
	}
	t.Fatal("no loopback port free for both TCP and UDP")
	return 0
}

// startShadowsocksServer runs an in-process sing-box Shadowsocks 2022 server
// that forwards to the destination directly, standing in for a real ingress.
func startShadowsocksServer(t *testing.T) uint16 {
	t.Helper()
	return startShadowsocksServerWithTracker(t, nil)
}

// startShadowsocksServerWithTracker is startShadowsocksServer with a tracker
// that sees every connection the server routes, including the destination
// the client asked for.
func startShadowsocksServerWithTracker(t *testing.T, tracker adapter.ConnectionTracker) uint16 {
	t.Helper()
	for attempt := 1; ; attempt++ {
		port, err := tryStartShadowsocksServer(t, tracker)
		if err == nil {
			return port
		}
		// Another socket took the port between freePort and the bind.
		if attempt == 3 || !strings.Contains(err.Error(), "address already in use") {
			t.Fatal(err)
		}
	}
}

func tryStartShadowsocksServer(t *testing.T, tracker adapter.ConnectionTracker) (uint16, error) {
	t.Helper()
	port := freePort(t)
	listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	ctx, cancel := context.WithCancel(context.Background())
	server, err := box.New(box.Options{Context: failover.Context(ctx), Options: option.Options{
		Log:      &option.LogOptions{Disabled: true},
		Inbounds: []option.Inbound{{Type: C.TypeShadowsocks, Tag: "ss-in", Options: &option.ShadowsocksInboundOptions{ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: port}, Method: "2022-blake3-aes-128-gcm", Password: testSSKey}}},
	}})
	if err != nil {
		cancel()
		return 0, err
	}
	if tracker != nil {
		server.Router().AppendTracker(tracker)
	}
	if err = server.Start(); err != nil {
		server.Close()
		cancel()
		return 0, err
	}
	t.Cleanup(func() { server.Close(); cancel() })
	return port, nil
}

func localSSIngress(role profile.IngressRole, key string, ordinal int, port uint16) profile.Ingress {
	return profile.Ingress{Role: role, EndpointKey: key, ReplicaOrdinal: ordinal, Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: "localhost", Port: port}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: testSSKey}}, Capabilities: profile.Capabilities{TCP: true}}
}

// TestLocalProxyOnlyCoreFailsOverToBackupIngress runs the real sing-box
// runtime the way the unprivileged desktop core does (--tun=false
// --local-proxy=true): a node whose primary ingress is down must still pass
// the availability ("Connect") test through its local proxy via the backup.
func TestLocalProxyOnlyCoreFailsOverToBackupIngress(t *testing.T) {
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }))
	defer target.Close()
	serverPort := startShadowsocksServer(t)
	deadPort := freePort(t)

	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "failover", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{
			{ID: "failover", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
				localSSIngress(profile.IngressRolePrimary, "f0", 0, deadPort),
				labeledIngress(localSSIngress(profile.IngressRoleBackup, "f1", 1, serverPort), "Relay F1"),
			}},
			{ID: "healthy", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
				localSSIngress(profile.IngressRolePrimary, "h0", 0, serverPort),
				localSSIngress(profile.IngressRoleBackup, "h1", 1, deadPort),
			}},
			{ID: "down", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
				localSSIngress(profile.IngressRolePrimary, "d0", 0, deadPort),
			}},
		},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "failover"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if _, err := core.ProbeAvailability(context.Background(), "failover", target.URL, time.Second); !errors.Is(err, ErrCoreNotRunning) {
		t.Fatalf("probe before start: %v", err)
	}
	if core.Status().SelectedIngress != nil {
		t.Fatal("selected ingress reported before start")
	}
	events := core.Subscribe(t.Context(), 16)
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	if ingress := core.Status().SelectedIngress; ingress == nil || ingress.EndpointKey != "f0" || ingress.Role != "primary" || ingress.Label != "" || ingress.PreviousEndpointKey != "" || ingress.SwitchedAt != nil {
		t.Fatalf("selected ingress before traffic: %#v", ingress)
	}
	for _, id := range []string{"failover", "healthy"} {
		result, err := core.ProbeAvailability(context.Background(), id, target.URL, 5*time.Second)
		if err != nil || !result.Success || result.HTTPStatus != http.StatusNoContent {
			t.Fatalf("%s: %v %#v", id, err, result)
		}
	}
	// The dead primary pushed the selected node's traffic to its backup.
	ingress := core.Status().SelectedIngress
	if ingress == nil || ingress.EndpointKey != "f1" || ingress.Label != "Relay F1" || ingress.PreviousEndpointKey != "f0" || ingress.Role != "backup" || ingress.SwitchedAt == nil {
		t.Fatalf("selected ingress after failover: %#v", ingress)
	}
	statusJSON, _ := json.Marshal(core.Status())
	if !strings.Contains(string(statusJSON), `"selected_ingress":{"endpoint_key":"f1","label":"Relay F1","previous_endpoint_key":"f0","role":"backup","switched_at":"`) {
		t.Fatalf("status JSON: %s", statusJSON)
	}
	switched := false
	for !switched {
		select {
		case event := <-events:
			switched = event.Type == EventNodeIngressSwitched && event.NodeID == "failover" && event.EndpointKey == "f1" && event.PreviousEndpointKey == "f0"
		case <-time.After(2 * time.Second):
			t.Fatal("no NodeIngressSwitched event")
		}
	}
	result, err := core.ProbeAvailability(context.Background(), "down", target.URL, 5*time.Second)
	if err != nil || result.Success {
		t.Fatalf("down node passed: %v %#v", err, result)
	}
	if _, err = core.ProbeAvailability(context.Background(), "missing", target.URL, time.Second); !errors.Is(err, ErrNodeNotFound) {
		t.Fatalf("missing node: %v", err)
	}
	// Selecting a logical node still works with failover groups underneath.
	if err = core.SelectNode("healthy"); err != nil {
		t.Fatal(err)
	}
	if ingress := core.Status().SelectedIngress; ingress == nil || ingress.EndpointKey != "h0" || ingress.Role != "primary" || ingress.PreviousEndpointKey != "" {
		t.Fatalf("selected ingress of healthy node: %#v", ingress)
	}
}

// TestTUNOnlyCoreRejectsLocalProxyAPIs covers `serve --tun --local-proxy=false`.
func TestTUNOnlyCoreRejectsLocalProxyAPIs(t *testing.T) {
	factory := &fakeFactory{}
	c := newCore(profile.PlatformCapabilities{Platform: "macos", TUN: profile.TUNCapabilities{Enabled: true}}, factory.create)
	if _, err := c.ApplyProfile(testProfile("r1", "a.example", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	if len(c.LocalProxyEndpoints()) != 0 {
		t.Fatal("local proxy endpoints created with local proxy disabled")
	}
	if _, err := c.ProbeAvailability(context.Background(), "node", "http://example.com", time.Second); !errors.Is(err, ErrLocalProxyDisabled) {
		t.Fatalf("availability: %v", err)
	}
	if _, err := c.LocalProxyCredential("node"); !errors.Is(err, ErrLocalProxyDisabled) {
		t.Fatalf("credential: %v", err)
	}
	// TUN cores report the selected node's ingress too.
	if ingress := c.Status().SelectedIngress; ingress == nil || ingress.EndpointKey != "9001" || ingress.Role != "primary" {
		t.Fatalf("TUN core selected ingress: %#v", ingress)
	}
	built := c.built.Options
	for _, inbound := range built.Inbounds {
		if inbound.Type != C.TypeTun {
			t.Fatalf("unexpected inbound %s", inbound.Type)
		}
	}
}

func labeledIngress(ingress profile.Ingress, label string) profile.Ingress {
	ingress.Label = &label
	return ingress
}

func TestFreePortIsFreeForTCPAndUDP(t *testing.T) {
	for range 20 {
		address := net.JoinHostPort("127.0.0.1", strconv.Itoa(int(freePort(t))))
		tcp, err := net.Listen("tcp", address)
		if err != nil {
			t.Fatal(err)
		}
		udp, err := net.ListenPacket("udp", address)
		tcp.Close()
		if err != nil {
			t.Fatal(err)
		}
		udp.Close()
	}
}
