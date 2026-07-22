package runtime

import (
	"context"
	"io"
	"net"
	"testing"

	"github.com/sagernet/sing-box/adapter"
)

func TestTelemetryCountsAndRemovesConnections(t *testing.T) {
	tracker := newTelemetry()
	left, right := net.Pipe()
	wrapped := tracker.RoutedConnection(context.Background(), left, adapter.InboundContext{Network: "tcp"}, nil, nil)
	writeDone := make(chan error, 1)
	go func() { _, err := wrapped.Write([]byte("up")); writeDone <- err }()
	buffer := make([]byte, 2)
	if _, err := io.ReadFull(right, buffer); err != nil {
		t.Fatal(err)
	}
	if err := <-writeDone; err != nil {
		t.Fatal(err)
	}
	go func() { _, _ = right.Write([]byte("down")) }()
	buffer = make([]byte, 4)
	if _, err := io.ReadFull(wrapped, buffer); err != nil {
		t.Fatal(err)
	}
	traffic, connections := tracker.snapshot()
	if traffic.UploadBytes != 2 || traffic.DownloadBytes != 4 || len(connections) != 1 {
		t.Fatalf("traffic=%#v connections=%#v", traffic, connections)
	}
	wrapped.Close()
	right.Close()
	_, connections = tracker.snapshot()
	if len(connections) != 0 {
		t.Fatal("closed connection retained")
	}
}
