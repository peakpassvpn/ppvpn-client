package main

import (
	"bytes"
	"context"
	"io"
	"net"
	"net/netip"
	"testing"
	"time"

	box "github.com/sagernet/sing-box"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/include"
	"github.com/sagernet/sing-box/option"
	M "github.com/sagernet/sing/common/metadata"
)

// Verify the real encrypted TCP and UDP fixture path, including the exact
// TEST-NET -> loopback route, before asking the emulator to consume it.
func TestSS2022RoutesTCPAndUDPToControlledSinks(t *testing.T) {
	tcp, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer tcp.Close()
	go echoTCP(tcp)
	udp, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer udp.Close()
	go echoUDP(udp)
	reservation, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := uint16(reservation.Addr().(*net.TCPAddr).Port)
	_ = reservation.Close()
	node, err := startNode(port)
	if err != nil {
		t.Fatal(err)
	}
	defer node.Close()
	ctx := box.Context(context.Background(), include.InboundRegistry(), include.OutboundRegistry(), include.EndpointRegistry(), include.DNSTransportRegistry(), include.ServiceRegistry())
	client, err := box.New(box.Options{Context: ctx, Options: option.Options{
		Log: &option.LogOptions{Disabled: true},
		Outbounds: []option.Outbound{{Type: C.TypeShadowsocks, Tag: "test", Options: &option.ShadowsocksOutboundOptions{
			ServerOptions: option.ServerOptions{Server: "127.0.0.1", ServerPort: port},
			Method:        "2022-blake3-aes-128-gcm", Password: "AAAAAAAAAAAAAAAAAAAAAA==",
		}}},
	}})
	if err != nil {
		t.Fatal(err)
	}
	if err = client.Start(); err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	outbound, ok := client.Outbound().Outbound("test")
	if !ok {
		t.Fatal("SS outbound missing")
	}
	for _, test := range []struct {
		network string
		port    int
	}{
		{"tcp", tcp.Addr().(*net.TCPAddr).Port},
		{"udp", udp.LocalAddr().(*net.UDPAddr).Port},
	} {
		t.Run(test.network, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			defer cancel()
			destination := M.Socksaddr{Addr: netip.MustParseAddr("192.0.2.10"), Port: uint16(test.port)}
			payload := []byte("actual-SS2022-" + test.network + "\n")
			var got []byte
			if test.network == "udp" {
				conn, e := outbound.ListenPacket(ctx, destination)
				if e != nil {
					t.Fatal(e)
				}
				defer conn.Close()
				_ = conn.SetDeadline(time.Now().Add(3 * time.Second))
				if _, e = conn.WriteTo(payload, destination.UDPAddr()); e != nil {
					t.Fatal(e)
				}
				packet := make([]byte, 65535)
				n, address, e := conn.ReadFrom(packet)
				if e != nil {
					t.Fatal(e)
				}
				if address.String() != destination.String() {
					t.Fatalf("fixture UDP source %s, want %s", address, destination)
				}
				got = packet[:n]
			} else {
				conn, e := outbound.DialContext(ctx, test.network, destination)
				if e != nil {
					t.Fatal(e)
				}
				defer conn.Close()
				_ = conn.SetDeadline(time.Now().Add(3 * time.Second))
				if _, e = conn.Write(payload); e != nil {
					t.Fatal(e)
				}
				got = make([]byte, len(payload))
				if _, e = io.ReadFull(conn, got); e != nil {
					t.Fatal(e)
				}
			}
			if !bytes.Equal(got, payload) {
				t.Fatalf("SS fixture wrong bytes: %x want %x", got, payload)
			}
		})
	}
}

func TestEchoSinksReturnActualBytes(t *testing.T) {
	tcp, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer tcp.Close()
	go echoTCP(tcp)
	stream, err := net.Dial("tcp", tcp.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	_ = stream.SetDeadline(time.Now().Add(2 * time.Second))
	for _, payload := range [][]byte{[]byte("cycle-0-tcp\n"), bytes.Repeat([]byte("fragmented-data-"), 1024)} {
		// Concurrent write/read exercises fragmented stream traffic too.
		write := make(chan error, 1)
		go func() { _, e := io.Copy(stream, bytes.NewReader(payload)); write <- e }()
		got := make([]byte, len(payload))
		if _, e := io.ReadFull(stream, got); e != nil {
			t.Fatal(e)
		}
		if e := <-write; e != nil {
			t.Fatal(e)
		}
		if !bytes.Equal(got, payload) {
			t.Fatal("TCP fixture corrupted bytes")
		}
	}
	udp, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer udp.Close()
	go echoUDP(udp)
	packet, err := net.Dial("udp", udp.LocalAddr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer packet.Close()
	_ = packet.SetDeadline(time.Now().Add(2 * time.Second))
	payload := []byte("cycle-0-udp")
	if _, e := packet.Write(payload); e != nil {
		t.Fatal(e)
	}
	got := make([]byte, 1024)
	n, e := packet.Read(got)
	if e != nil {
		t.Fatal(e)
	}
	if !bytes.Equal(got[:n], payload) {
		t.Fatal("UDP fixture corrupted datagram")
	}
}
