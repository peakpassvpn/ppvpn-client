package probe

import (
	"context"
	"errors"
	"net"
	"net/netip"
	"sync"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

func ssIngress(role profile.IngressRole, domain, ip string) profile.Ingress {
	return profile.Ingress{Role: role, EndpointKey: domain, Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: domain, IP: ip, Port: 443}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}}, Capabilities: profile.Capabilities{TCP: true}}
}

func probeProfile(ingresses ...profile.Ingress) *profile.Profile {
	if len(ingresses) == 0 {
		ingresses = []profile.Ingress{ssIngress(profile.IngressRolePrimary, "must-not-resolve.invalid", "8.8.8.8")}
	}
	for i := range ingresses {
		ingresses[i].ReplicaOrdinal = i
	}
	n := profile.Node{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: ingresses}
	return &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "r", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{n}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
}

func TestEntranceUsesLiteralIP(t *testing.T) {
	server, client := net.Pipe()
	defer server.Close()
	dial := func(_ context.Context, network, address string) (net.Conn, error) {
		if address != "8.8.8.8:443" {
			t.Errorf("used %s", address)
		}
		return client, nil
	}
	resolve := func(context.Context, string) ([]netip.Addr, error) {
		t.Error("resolved despite literal IP")
		return nil, errors.New("unexpected")
	}
	got, err := Entrances(context.Background(), probeProfile(), Options{Timeout: time.Second, Concurrency: 1, Dial: dial, Resolve: resolve})
	if err != nil || !got[0].Success || got[0].Method != MethodTCP || got[0].IngressRole != profile.IngressRolePrimary || len(got[0].Ingresses) != 1 {
		t.Fatalf("%v %#v", err, got)
	}
}

func TestEntranceResolvesDomainWithoutIP(t *testing.T) {
	var mu sync.Mutex
	var dialed []string
	dial := func(_ context.Context, _, address string) (net.Conn, error) {
		mu.Lock()
		dialed = append(dialed, address)
		mu.Unlock()
		a, b := net.Pipe()
		b.Close()
		return a, nil
	}
	resolve := func(_ context.Context, host string) ([]netip.Addr, error) {
		if host != "edge.example.com" {
			t.Errorf("resolved %s", host)
		}
		return []netip.Addr{netip.MustParseAddr("2001:4860::8888"), netip.MustParseAddr("9.9.9.9")}, nil
	}
	got, err := Entrances(context.Background(), probeProfile(ssIngress(profile.IngressRolePrimary, "edge.example.com", "")), Options{Dial: dial, Resolve: resolve})
	if err != nil || !got[0].Success || len(dialed) != 1 || dialed[0] != "9.9.9.9:443" {
		t.Fatalf("%v %#v %v", err, got, dialed)
	}
}

func TestEntranceDNSFailure(t *testing.T) {
	resolve := func(context.Context, string) ([]netip.Addr, error) { return nil, errors.New("nxdomain") }
	got, err := Entrances(context.Background(), probeProfile(ssIngress(profile.IngressRolePrimary, "edge.example.com", "")), Options{Method: MethodICMP, Resolve: resolve})
	if err != nil || got[0].Success || got[0].ErrorCode != CodeDNSFailed {
		t.Fatalf("%v %#v", err, got)
	}
}

func TestEntranceFallsBackToBestBackup(t *testing.T) {
	p := probeProfile(
		ssIngress(profile.IngressRolePrimary, "a.example.com", "8.8.8.8"),
		ssIngress(profile.IngressRoleBackup, "b.example.com", "1.1.1.1"),
		ssIngress(profile.IngressRoleBackup, "c.example.com", "9.9.9.9"),
	)
	ping := func(_ context.Context, addr netip.Addr, _ time.Duration) (time.Duration, error) {
		switch addr.String() {
		case "8.8.8.8":
			return 0, &Error{Code: CodeICMPTimeout}
		case "1.1.1.1":
			return 80 * time.Millisecond, nil
		default:
			return 30 * time.Millisecond, nil
		}
	}
	got, err := Entrances(context.Background(), p, Options{Method: MethodICMP, Ping: ping})
	if err != nil {
		t.Fatal(err)
	}
	r := got[0]
	if !r.Success || r.LatencyMS != 30 || r.IngressRole != profile.IngressRoleBackup || r.EndpointKey != "c.example.com" || r.Method != MethodICMP || r.ErrorCode != "" {
		t.Fatalf("%#v", r)
	}
	if r.Ingresses[0].Success || r.Ingresses[0].ErrorCode != CodeICMPTimeout || r.Ingresses[0].Role != profile.IngressRolePrimary || !r.Ingresses[2].Success ||
		r.Ingresses[0].EndpointKey != "a.example.com" || r.Ingresses[2].EndpointKey != "c.example.com" || r.Ingresses[2].ReplicaOrdinal != 2 {
		t.Fatalf("%#v", r.Ingresses)
	}
}

func TestEntrancePrimaryWinsWhenHealthy(t *testing.T) {
	p := probeProfile(
		ssIngress(profile.IngressRolePrimary, "a.example.com", "8.8.8.8"),
		ssIngress(profile.IngressRoleBackup, "b.example.com", "1.1.1.1"),
	)
	ping := func(_ context.Context, addr netip.Addr, _ time.Duration) (time.Duration, error) {
		if addr.String() == "8.8.8.8" {
			return 90 * time.Millisecond, nil
		}
		return 10 * time.Millisecond, nil
	}
	got, err := Entrances(context.Background(), p, Options{Method: MethodICMP, Ping: ping})
	if err != nil || got[0].IngressRole != profile.IngressRolePrimary || got[0].EndpointKey != "a.example.com" || got[0].LatencyMS != 90 {
		t.Fatalf("%v %#v", err, got)
	}
}

func TestEntranceAllFailedReportsPrimary(t *testing.T) {
	p := probeProfile(
		ssIngress(profile.IngressRolePrimary, "a.example.com", "8.8.8.8"),
		ssIngress(profile.IngressRoleBackup, "b.example.com", "1.1.1.1"),
	)
	dial := func(context.Context, string, string) (net.Conn, error) { return nil, errors.New("refused") }
	got, err := Entrances(context.Background(), p, Options{Dial: dial})
	if err != nil || got[0].Success || got[0].ErrorCode != CodeConnectFailed || got[0].IngressRole != profile.IngressRolePrimary {
		t.Fatalf("%v %#v", err, got)
	}
}

func TestEntranceTimeout(t *testing.T) {
	dial := func(ctx context.Context, _, _ string) (net.Conn, error) { <-ctx.Done(); return nil, ctx.Err() }
	got, err := Entrances(context.Background(), probeProfile(), Options{Timeout: time.Millisecond, Concurrency: 1, Dial: dial})
	if err != nil || got[0].ErrorCode != CodeTimeout {
		t.Fatalf("%v %#v", err, got)
	}
}

func TestEntranceCanceled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	got, err := Entrances(ctx, probeProfile(), Options{Timeout: time.Second, Concurrency: 1})
	if err != nil || got[0].ErrorCode != CodeCanceled {
		t.Fatalf("%v %#v", err, got)
	}
}

func TestParseMethod(t *testing.T) {
	for in, want := range map[string]Method{"": MethodTCP, "tcp": MethodTCP, "icmp": MethodICMP} {
		if got, err := ParseMethod(in); err != nil || got != want {
			t.Fatalf("%q: %v %v", in, got, err)
		}
	}
	if _, err := ParseMethod("udp"); err == nil {
		t.Fatal("udp accepted")
	}
	if _, err := Entrances(context.Background(), probeProfile(), Options{Method: "http"}); err == nil {
		t.Fatal("unknown method accepted")
	}
}
