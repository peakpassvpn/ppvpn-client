package runtime

import (
	"context"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/profile"
	box "github.com/sagernet/sing-box"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

const testSSKey = "AAAAAAAAAAAAAAAAAAAAAA=="

func freePort(t *testing.T) uint16 {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	return uint16(l.Addr().(*net.TCPAddr).Port)
}

// startShadowsocksServer runs an in-process sing-box Shadowsocks 2022 server
// that forwards to the destination directly, standing in for a real ingress.
func startShadowsocksServer(t *testing.T) uint16 {
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
		t.Fatal(err)
	}
	if err = server.Start(); err != nil {
		cancel()
		t.Fatal(err)
	}
	t.Cleanup(func() { server.Close(); cancel() })
	return port
}

func localSSIngress(role profile.IngressRole, key string, ordinal int, port uint16) profile.Ingress {
	return profile.Ingress{Role: role, EndpointKey: key, ReplicaOrdinal: ordinal, Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: "localhost", Port: port}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: testSSKey}}, Capabilities: profile.Capabilities{TCP: true}}
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
				localSSIngress(profile.IngressRoleBackup, "f1", 1, serverPort),
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
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	for _, id := range []string{"failover", "healthy"} {
		result, err := core.ProbeAvailability(context.Background(), id, target.URL, 5*time.Second)
		if err != nil || !result.Success || result.HTTPStatus != http.StatusNoContent {
			t.Fatalf("%s: %v %#v", id, err, result)
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
	built := c.built.Options
	for _, inbound := range built.Inbounds {
		if inbound.Type != C.TypeTun {
			t.Fatalf("unexpected inbound %s", inbound.Type)
		}
	}
}
