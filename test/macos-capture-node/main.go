// Test-only real SS 2022 upstream. It is a separate loopback node, never a
// PPVPN forwarding owner, and reports decoded upstream/target accept counts.
package main

import (
	"context"
	"encoding/json"
	"io"
	"log"
	"net"
	"net/http"
	"net/netip"
	"os"
	"sync/atomic"

	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/include"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
	N "github.com/sagernet/sing/common/network"
)

type counters struct {
	upstream    atomic.Uint64
	accepts     atomic.Uint64
	bytes       atomic.Uint64
	target      string
	udpTarget   string
	udpUpstream atomic.Uint64
	udpPackets  atomic.Uint64
}

func (c *counters) RoutedConnection(_ context.Context, conn net.Conn, m adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) net.Conn {
	if m.Destination.String() == c.target {
		c.upstream.Add(1)
	}
	return conn
}
func (c *counters) RoutedPacketConnection(_ context.Context, conn N.PacketConn, m adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) N.PacketConn {
	if m.Destination.String() == c.udpTarget {
		c.udpUpstream.Add(1)
	}
	return conn
}

func listen() net.Listener {
	l, e := net.Listen("tcp", "127.0.0.1:0")
	if e != nil {
		log.Fatal(e)
	}
	return l
}

func main() {
	sink, stats, reservation := listen(), listen(), listen()
	ssPort := uint16(reservation.Addr().(*net.TCPAddr).Port)
	_ = reservation.Close()
	udp, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		log.Fatal(err)
	}
	defer udp.Close()
	c := &counters{target: sink.Addr().String(), udpTarget: udp.LocalAddr().String()}
	go func() {
		packet := make([]byte, 2048)
		for {
			n, peer, e := udp.ReadFrom(packet)
			if e != nil {
				return
			}
			c.udpPackets.Add(1)
			if _, e = udp.WriteTo(packet[:n], peer); e != nil {
				return
			}
		}
	}()
	go func() {
		for {
			conn, err := sink.Accept()
			if err != nil {
				return
			}
			c.accepts.Add(1)
			go func() {
				defer conn.Close()
				buf := make([]byte, 4096)
				for {
					n, err := conn.Read(buf)
					if n > 0 {
						c.bytes.Add(uint64(n))
						if _, e := conn.Write(buf[:n]); e != nil {
							return
						}
					}
					if err != nil {
						return
					}
				}
			}()
		}
	}()
	go func() {
		_ = http.Serve(stats, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			_ = json.NewEncoder(w).Encode(map[string]uint64{
				"upstream_connections": c.upstream.Load(), "sink_accepts": c.accepts.Load(), "sink_bytes": c.bytes.Load(), "udp_upstream_associations": c.udpUpstream.Load(), "udp_sink_packets": c.udpPackets.Load(),
			})
		}))
	}()
	listenIP := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	ctx := context.Background()
	server, err := box.New(box.Options{Context: box.Context(ctx, include.InboundRegistry(), include.OutboundRegistry(), include.EndpointRegistry(), include.DNSTransportRegistry(), include.ServiceRegistry()), Options: option.Options{
		Log: &option.LogOptions{Disabled: true},
		Inbounds: []option.Inbound{{Type: C.TypeShadowsocks, Tag: "capture-qa-ss", Options: &option.ShadowsocksInboundOptions{
			ListenOptions: option.ListenOptions{Listen: &listenIP, ListenPort: ssPort}, Method: "2022-blake3-aes-128-gcm", Password: "AAAAAAAAAAAAAAAAAAAAAA==",
		}}},
		Outbounds: []option.Outbound{{Type: C.TypeDirect, Tag: "direct"}},
	}})
	if err != nil {
		log.Fatal(err)
	}
	server.Router().AppendTracker(c)
	if err := server.Start(); err != nil {
		log.Fatal(err)
	}
	defer server.Close()
	_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"ss_port": ssPort, "sink": sink.Addr().String(), "stats": stats.Addr().String(), "udp_sink": udp.LocalAddr().String()})
	_, _ = io.Copy(io.Discard, os.Stdin)
}
