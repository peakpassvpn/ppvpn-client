package probe

import (
	"context"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"net"
	"testing"
	"time"
)

func probeProfile() *profile.Profile {
	n := profile.Node{ID: "node", Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: "must-not-resolve.invalid", IP: "8.8.8.8", Port: 443}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}}, Capabilities: profile.Capabilities{TCP: true}}
	return &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: "r", ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{n}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
}
func TestEntranceUsesLiteralIP(t *testing.T) {
	server, client := net.Pipe()
	defer server.Close()
	dial := func(_ context.Context, network, address string) (net.Conn, error) {
		if address != "8.8.8.8:443" {
			t.Fatalf("used %s", address)
		}
		return client, nil
	}
	got, err := Entrances(context.Background(), probeProfile(), time.Second, 1, dial)
	if err != nil || !got[0].Success {
		t.Fatalf("%v %#v", err, got)
	}
}
func TestEntranceTimeout(t *testing.T) {
	dial := func(ctx context.Context, _, _ string) (net.Conn, error) { <-ctx.Done(); return nil, ctx.Err() }
	got, err := Entrances(context.Background(), probeProfile(), time.Millisecond, 1, dial)
	if err != nil || got[0].ErrorCode != "TIMEOUT" {
		t.Fatalf("%v %#v", err, got)
	}
}
func TestEntranceCanceled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	got, err := Entrances(ctx, probeProfile(), time.Second, 1, nil)
	if err != nil || got[0].ErrorCode != "CANCELED" {
		t.Fatalf("%v %#v", err, got)
	}
}
