// Command fakenode is the proxy node of the desktop's Linux enhanced-mode
// end-to-end test (crates/desktop/src/e2e_linux.rs): a
// Shadowsocks 2022 server (sing-box's stock registries), plus a plain HTTP
// target. It runs in run.sh's uplink namespace (ppvpn-w); nothing in it
// reaches beyond the machine.
//
// It prints one line per event on stdout:
//
//	ready ss=<addr> http=<addr>
//	conn <destination>          (every connection the node routes)
//
// Test-only: no release build includes it.
package main

import (
	"context"
	"flag"
	"fmt"
	"net"
	"net/http"
	"net/netip"
	"os"
	"os/signal"
	"sync"
	"syscall"

	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/include"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
	N "github.com/sagernet/sing/common/network"
)

// HTTPBody is what the HTTP target answers; the test checks for it.
const HTTPBody = "ppvpn-e2e-ok\n"

var stdout sync.Mutex

func emit(format string, args ...any) {
	stdout.Lock()
	defer stdout.Unlock()
	fmt.Printf(format+"\n", args...)
}

// printer reports the destination of every routed connection.
type printer struct{}

func (printer) RoutedConnection(_ context.Context, conn net.Conn, metadata adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) net.Conn {
	emit("conn %s", metadata.Destination.String())
	return conn
}

func (printer) RoutedPacketConnection(_ context.Context, conn N.PacketConn, metadata adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) N.PacketConn {
	emit("conn udp:%s", metadata.Destination.String())
	return conn
}

func main() {
	ssAddr := flag.String("ss", "", "Shadowsocks listen address (ip:port)")
	method := flag.String("method", "2022-blake3-aes-128-gcm", "Shadowsocks method")
	key := flag.String("key", "", "Shadowsocks key (base64)")
	httpAddr := flag.String("http", "", "HTTP target listen address (ip:port)")
	flag.Parse()
	if *ssAddr == "" || *key == "" || *httpAddr == "" {
		fmt.Fprintln(os.Stderr, "usage: fakenode -ss ip:port -key base64 -http ip:port")
		os.Exit(2)
	}
	ss, err := netip.ParseAddrPort(*ssAddr)
	if err != nil {
		fmt.Fprintln(os.Stderr, "fakenode: -ss:", err)
		os.Exit(2)
	}

	target, err := net.Listen("tcp", *httpAddr)
	if err != nil {
		fmt.Fprintln(os.Stderr, "fakenode: http target:", err)
		os.Exit(1)
	}
	go func() {
		_ = http.Serve(target, http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			_, _ = w.Write([]byte(HTTPBody))
		}))
	}()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	listen := badoption.Addr(ss.Addr())
	server, err := box.New(box.Options{Context: box.Context(ctx, include.InboundRegistry(), include.OutboundRegistry(), include.EndpointRegistry(), include.DNSTransportRegistry(), include.ServiceRegistry()), Options: option.Options{
		Log: &option.LogOptions{Disabled: true},
		Inbounds: []option.Inbound{{
			Type: C.TypeShadowsocks,
			Tag:  "ss-in",
			Options: &option.ShadowsocksInboundOptions{
				ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: ss.Port()},
				Method:        *method,
				Password:      *key,
			},
		}},
	}})
	if err != nil {
		fmt.Fprintln(os.Stderr, "fakenode: shadowsocks server:", err)
		os.Exit(1)
	}
	server.Router().AppendTracker(printer{})
	if err := server.Start(); err != nil {
		fmt.Fprintln(os.Stderr, "fakenode: start:", err)
		os.Exit(1)
	}
	defer server.Close()
	emit("ready ss=%s http=%s", ss, target.Addr())

	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGINT, syscall.SIGTERM)
	<-signals
}
