// Android emulator acceptance only. SS2022 is reachable at the emulator host
// alias; TEST-NET TCP/UDP destinations are redirected to this host's loopback.
// DNS keeps the production DoT path through SS; it needs outbound TCP/853.
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"os/signal"
	"sync"
	"syscall"

	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/include"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
	N "github.com/sagernet/sing/common/network"
)

var output sync.Mutex

func emit(event map[string]any) {
	output.Lock()
	defer output.Unlock()
	_ = json.NewEncoder(os.Stdout).Encode(event)
}

type tracker struct{}

func (tracker) RoutedConnection(_ context.Context, c net.Conn, m adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) net.Conn {
	emit(map[string]any{"event": "route", "network": "tcp", "destination": m.Destination.String()})
	return c
}
func (tracker) RoutedPacketConnection(_ context.Context, c N.PacketConn, m adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) N.PacketConn {
	emit(map[string]any{"event": "route", "network": "udp", "destination": m.Destination.String()})
	return c
}

func main() {
	port := flag.Uint("port", 18388, "loopback SS2022 port")
	flag.Parse()
	if *port == 0 || *port > 65535 {
		panic("invalid SS port")
	}
	tcp, err := net.Listen("tcp", "127.0.0.1:18081")
	if err != nil {
		panic(err)
	}
	defer tcp.Close()
	udp, err := net.ListenPacket("udp", "127.0.0.1:18082")
	if err != nil {
		panic(err)
	}
	defer udp.Close()
	go echoTCP(tcp)
	go echoUDP(udp)
	instance, err := startNode(uint16(*port))
	if err != nil {
		panic(err)
	}
	defer instance.Close()
	emit(map[string]any{"event": "ready", "ss_port": *port})
	end := make(chan os.Signal, 1)
	signal.Notify(end, syscall.SIGTERM, syscall.SIGINT)
	<-end
}

func startNode(port uint16) (*box.Box, error) {
	listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	ctx := box.Context(context.Background(), include.InboundRegistry(), include.OutboundRegistry(), include.EndpointRegistry(), include.DNSTransportRegistry(), include.ServiceRegistry())
	instance, err := box.New(box.Options{Context: ctx, Options: option.Options{
		Log: &option.LogOptions{Level: "debug", Output: "stderr", Timestamp: true, DisableColor: true},
		Inbounds: []option.Inbound{{Type: C.TypeShadowsocks, Tag: "android-acceptance", Options: &option.ShadowsocksInboundOptions{
			ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: port},
			Method:        "2022-blake3-aes-128-gcm", Password: "AAAAAAAAAAAAAAAAAAAAAA==",
		}}},
		Outbounds: []option.Outbound{{Type: C.TypeDirect, Tag: "direct"}},
		Route: &option.RouteOptions{Rules: []option.Rule{{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{
			RawDefaultRule: option.RawDefaultRule{IPCIDR: badoption.Listable[string]{"192.0.2.10/32"}},
			RuleAction:     option.RuleAction{Action: C.RuleActionTypeRouteOptions, RouteOptionsOptions: option.RouteOptionsActionOptions{OverrideAddress: "127.0.0.1"}},
		}}}},
	}})
	if err != nil {
		return nil, err
	}
	instance.Router().AppendTracker(tracker{})
	if err = instance.Start(); err != nil {
		return nil, err
	}
	return instance, nil
}

func echoTCP(listener net.Listener) {
	for {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		go func() {
			defer conn.Close()
			// The line sent by the device is also independent upstream evidence.
			buf := make([]byte, 4096)
			for {
				n, err := conn.Read(buf)
				if n > 0 {
					emit(map[string]any{"event": "echo", "network": "tcp", "payload": string(buf[:n])})
					if _, e := io.CopyN(conn, bytes.NewReader(buf[:n]), int64(n)); e != nil {
						return
					}
				}
				if err != nil {
					return
				}
			}
		}()
	}
}

func echoUDP(conn net.PacketConn) {
	buf := make([]byte, 65535)
	for {
		n, address, err := conn.ReadFrom(buf)
		if err != nil {
			return
		}
		emit(map[string]any{"event": "echo", "network": "udp", "payload": string(buf[:n])})
		if _, err = conn.WriteTo(buf[:n], address); err != nil {
			fmt.Fprintln(os.Stderr, err)
		}
	}
}
