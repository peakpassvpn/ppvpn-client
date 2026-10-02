package runtime

import (
	"bytes"
	"context"
	"io"
	"net"
	"testing"

	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing/common/buf"
	"github.com/sagernet/sing/common/bufio"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
)

// The routed conn is the inbound (client) side, as sing-box hands it to the
// tracker: what the router reads from it is the client's upload, what it
// writes to it is the download back to the client.
func TestTelemetryCountsAndRemovesConnections(t *testing.T) {
	tracker := newTelemetry()
	inbound, client := net.Pipe()
	wrapped := tracker.RoutedConnection(context.Background(), inbound, adapter.InboundContext{Network: "tcp"}, nil, nil)
	go func() { _, _ = client.Write([]byte("up")) }() // client -> remote
	buffer := make([]byte, 2)
	if _, err := io.ReadFull(wrapped, buffer); err != nil {
		t.Fatal(err)
	}
	writeDone := make(chan error, 1)
	go func() { _, err := wrapped.Write([]byte("down")); writeDone <- err }() // remote -> client
	buffer = make([]byte, 4)
	if _, err := io.ReadFull(client, buffer); err != nil {
		t.Fatal(err)
	}
	if err := <-writeDone; err != nil {
		t.Fatal(err)
	}
	traffic, connections := tracker.snapshot()
	if traffic.UploadBytes != 2 || traffic.DownloadBytes != 4 || len(connections) != 1 ||
		connections[0].UploadBytes != 2 || connections[0].DownloadBytes != 4 {
		t.Fatalf("traffic=%#v connections=%#v", traffic, connections)
	}
	wrapped.Close()
	client.Close()
	_, connections = tracker.snapshot()
	if len(connections) != 0 {
		t.Fatal("closed connection retained")
	}
}

// A sniffed connection reaches the tracker as a bufio.CachedConn holding the
// sniffed bytes. Behind the counting wrappers sing-box's copy loop reads it
// through CachedConn.Read, which clears the buffer unsynchronized while
// Close (from the other copy direction) reads it: a data race that can
// release the pooled buffer twice. The tracker takes the cached bytes itself,
// so reads and a concurrent close never share the buffer.
func TestTrackedSniffedConnectionReadRacesClose(t *testing.T) {
	for range 50 {
		client, server := net.Pipe()
		sniffed := buf.New()
		_, _ = sniffed.Write([]byte("GET / HTTP/1.1\r\n"))
		cached := bufio.NewCachedConn(server, sniffed)
		sniffed.Release() // the router's own reference
		conn := newTelemetry().RoutedConnection(context.Background(), cached, adapter.InboundContext{}, nil, nil)
		done := make(chan []byte)
		go func() {
			got, _ := io.ReadAll(conn)
			done <- got
		}()
		go func() { _, _ = client.Write([]byte("Host: a\r\n")); client.Close() }()
		_ = conn.Close()
		got := <-done
		if len(got) > 0 && !bytes.HasPrefix(got, []byte("GET / HTTP/1.1\r\n")) {
			t.Fatalf("cached bytes lost or reordered: %q", got)
		}
	}
}

// Without a concurrent close the cached bytes come first, then the stream.
func TestTrackedSniffedConnectionKeepsCachedBytes(t *testing.T) {
	client, server := net.Pipe()
	sniffed := buf.New()
	_, _ = sniffed.Write([]byte("hello "))
	cached := bufio.NewCachedConn(server, sniffed)
	sniffed.Release()
	conn := newTelemetry().RoutedConnection(context.Background(), cached, adapter.InboundContext{}, nil, nil)
	go func() { _, _ = client.Write([]byte("world")); client.Close() }()
	got, err := io.ReadAll(conn)
	if err != nil || string(got) != "hello world" {
		t.Fatalf("got %q, %v", got, err)
	}
	_ = conn.Close()
}

type eofPacketConn struct{ N.PacketConn }

func (eofPacketConn) ReadPacket(*buf.Buffer) (M.Socksaddr, error) { return M.Socksaddr{}, io.EOF }
func (eofPacketConn) Close() error                                { return nil }

// A sniffed UDP flow keeps its first (cached) packet and destination.
func TestTrackedSniffedPacketConnectionKeepsCachedPacket(t *testing.T) {
	sniffed := buf.New()
	_, _ = sniffed.Write([]byte("query"))
	destination := M.ParseSocksaddr("10.0.0.1:53")
	cached := bufio.NewCachedPacketConn(eofPacketConn{}, sniffed, destination)
	sniffed.Release()
	conn := newTelemetry().RoutedPacketConnection(context.Background(), cached, adapter.InboundContext{}, nil, nil)
	defer conn.Close()
	packet := buf.New()
	defer packet.Release()
	got, err := conn.ReadPacket(packet)
	if err != nil || got != destination || string(packet.Bytes()) != "query" {
		t.Fatalf("first packet %q to %v: %v", packet.Bytes(), got, err)
	}
	if _, err = conn.ReadPacket(buf.New()); err != io.EOF {
		t.Fatalf("second read: %v", err)
	}
}
