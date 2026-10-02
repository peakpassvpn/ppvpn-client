package runtime

import (
	"bufio"
	"bytes"
	"context"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/protocol/socks"
)

// TestTrafficDirection: upload is what the client sends toward the remote,
// download is what the remote returns. A small request for a large response
// through the shared local proxy and a node must count mostly download, in
// get-traffic and on the connection, for TCP and UDP. (Before 0.5.16 the
// two were swapped.)
func TestTrafficDirection(t *testing.T) {
	const responseSize = 1 << 20
	body := bytes.Repeat([]byte("d"), responseSize)
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write(body) }))
	defer target.Close()
	// UDP: one small request is answered with 64 datagrams of 1000 bytes.
	echo, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer echo.Close()
	go func() {
		buffer := make([]byte, 2048)
		for {
			_, from, err := echo.ReadFrom(buffer)
			if err != nil {
				return
			}
			reply := bytes.Repeat([]byte("u"), 1000)
			for range 64 {
				_, _ = echo.WriteTo(reply, from)
			}
		}
	}()

	ingress := localSSIngress(profile.IngressRolePrimary, "n0", 0, startShadowsocksServer(t))
	ingress.Capabilities.UDP = true
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "traffic-1", ExpiresAt: time.Now().Add(time.Hour),
		Nodes:     []profile.Node{{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true, UDP: true}, Ingresses: []profile.Ingress{ingress}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err = core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err = core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	endpoint := core.LocalProxyEndpoints()[0]

	// TCP: CONNECT, a ~100-byte GET, a 1 MiB response; check the open
	// connection, then the totals.
	before := core.Traffic()
	tunnel, status, err := httpConnect(endpoint, endpoint.Username, endpoint.Password, target.Listener.Addr().String())
	if err != nil || status[:3] != "200" {
		t.Fatalf("CONNECT: %q %v", status, err)
	}
	defer tunnel.Close()
	if _, err = io.WriteString(tunnel, "GET / HTTP/1.1\r\nHost: t\r\n\r\n"); err != nil {
		t.Fatal(err)
	}
	response, err := http.ReadResponse(bufio.NewReader(tunnel), nil)
	if err != nil {
		t.Fatal(err)
	}
	if n, err := io.Copy(io.Discard, response.Body); err != nil || n != responseSize {
		t.Fatalf("body %d %v", n, err)
	}
	var connection *Connection
	for deadline := time.Now().Add(3 * time.Second); connection == nil && time.Now().Before(deadline); {
		for _, c := range core.Connections() {
			if c.Network == "tcp" && c.DownloadBytes >= responseSize {
				connection = &c
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	if connection == nil {
		t.Fatalf("no connection with the download counted: %#v", core.Connections())
	}
	if connection.UploadBytes == 0 || connection.UploadBytes > 4096 {
		t.Fatalf("connection upload %d, want the small request", connection.UploadBytes)
	}
	after := core.Traffic()
	up, down := after.UploadBytes-before.UploadBytes, after.DownloadBytes-before.DownloadBytes
	if down < responseSize || up > 4096 {
		t.Fatalf("tcp traffic up=%d down=%d, want down >= %d and up small", up, down, responseSize)
	}

	// UDP through SOCKS5 UDP ASSOCIATE: 1 small datagram out, 64 KB back.
	before = core.Traffic()
	client := socks.NewClient(N.SystemDialer, M.ParseSocksaddrHostPort(endpoint.Listen, endpoint.Port), socks.Version5, endpoint.Username, endpoint.Password)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	destination := M.SocksaddrFromNet(echo.LocalAddr())
	packets, err := client.ListenPacket(ctx, destination)
	if err != nil {
		t.Fatal(err)
	}
	defer packets.Close()
	if _, err = packets.WriteTo([]byte("ping"), destination.UDPAddr()); err != nil {
		t.Fatal(err)
	}
	_ = packets.SetReadDeadline(time.Now().Add(3 * time.Second))
	received := 0
	buffer := make([]byte, 2048)
	for received < 32*1000 {
		n, _, err := packets.ReadFrom(buffer)
		if err != nil {
			t.Fatalf("udp after %d bytes: %v", received, err)
		}
		received += n
	}
	time.Sleep(200 * time.Millisecond) // let the rest arrive and be counted
	after = core.Traffic()
	up, down = after.UploadBytes-before.UploadBytes, after.DownloadBytes-before.DownloadBytes
	if down < uint64(received) || up > 1024 {
		t.Fatalf("udp traffic up=%d down=%d (received %d), want down >= received and up small", up, down, received)
	}
}
