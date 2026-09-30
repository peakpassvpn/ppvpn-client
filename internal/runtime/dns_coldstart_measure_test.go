package runtime

import (
	"context"
	"crypto/tls"
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"os"
	"sort"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/profile"
	box "github.com/sagernet/sing-box"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

// TestMeasureDNSColdStart measures how long the first hijacked DNS queries
// take through the node for each remote DNS transport, with a simulated
// client-to-node round trip. It only runs with PPVPN_MEASURE_DNS=1 and prints
// a table; it asserts nothing.
func TestMeasureDNSColdStart(t *testing.T) {
	if os.Getenv("PPVPN_MEASURE_DNS") != "1" {
		t.Skip("set PPVPN_MEASURE_DNS=1 to measure")
	}
	oneWay := 100 * time.Millisecond
	if v := os.Getenv("PPVPN_MEASURE_ONE_WAY_MS"); v != "" {
		ms, _ := strconv.Atoi(v)
		oneWay = time.Duration(ms) * time.Millisecond
	}
	ssPort := startShadowsocksServerUDP(t)
	relayPort := startDelayRelay(t, ssPort, oneWay)
	resolver := startMeasureResolvers(t)

	type result struct {
		name       string
		first      time.Duration
		burst      []time.Duration
		afterBurst time.Duration
	}
	var results []result
	for _, transport := range []string{"tls", "tcp", "udp", "https"} {
		for _, warm := range []bool{false, true} {
			name := transport
			if warm {
				name += "+warmup"
			}
			r := result{name: name}
			func() {
				socksPort, boxCtx, cleanup := startMeasureCore(t, relayPort, transport, resolver)
				defer cleanup()
				if warm {
					warmUpRemoteDNS(boxCtx)
				}
				r.first = hijackedQuery(t, socksPort, "a.test.")
				var wg sync.WaitGroup
				var mu sync.Mutex
				for i := range 4 {
					wg.Add(1)
					go func() {
						defer wg.Done()
						d := hijackedQuery(t, socksPort, fmt.Sprintf("b%d.test.", i))
						mu.Lock()
						r.burst = append(r.burst, d)
						mu.Unlock()
					}()
				}
				wg.Wait()
				sort.Slice(r.burst, func(i, j int) bool { return r.burst[i] < r.burst[j] })
				r.afterBurst = hijackedQuery(t, socksPort, "c.test.")
			}()
			results = append(results, r)
		}
	}
	t.Logf("simulated client<->node RTT %s (node<->resolver local)", 2*oneWay)
	t.Logf("%-14s %10s %22s %10s", "transport", "first", "4 concurrent (min/max)", "next")
	for _, r := range results {
		t.Logf("%-14s %10s %10s/%-11s %10s", r.name, r.first.Round(time.Millisecond), r.burst[0].Round(time.Millisecond), r.burst[len(r.burst)-1].Round(time.Millisecond), r.afterBurst.Round(time.Millisecond))
	}

	// What the OS actually does: a burst of queries right after start,
	// racing the background warm-up the core starts at the same moment.
	t.Logf("%-14s %22s", "cold burst", "4 concurrent (min/max)")
	for _, transport := range []string{"tls", "https"} {
		for _, warm := range []bool{false, true} {
			name := transport
			if warm {
				name += "+bg-warmup"
			}
			func() {
				socksPort, boxCtx, cleanup := startMeasureCore(t, relayPort, transport, resolver)
				defer cleanup()
				if warm {
					go warmUpRemoteDNS(boxCtx)
				}
				var wg sync.WaitGroup
				var mu sync.Mutex
				var burst []time.Duration
				for i := range 4 {
					wg.Add(1)
					go func() {
						defer wg.Done()
						d := hijackedQuery(t, socksPort, fmt.Sprintf("d%d.test.", i))
						mu.Lock()
						burst = append(burst, d)
						mu.Unlock()
					}()
				}
				wg.Wait()
				sort.Slice(burst, func(i, j int) bool { return burst[i] < burst[j] })
				t.Logf("%-14s %10s/%-11s", name, burst[0].Round(time.Millisecond), burst[len(burst)-1].Round(time.Millisecond))
			}()
		}
	}
}

type measureResolvers struct{ tcp, udp, tls, https uint16 }

// startMeasureResolvers serves A 203.0.113.9 for every name over TCP, UDP,
// DoT and DoH (HTTP/2), the last two with a self-signed certificate.
func startMeasureResolvers(t *testing.T) measureResolvers {
	t.Helper()
	handler := dns.HandlerFunc(func(w dns.ResponseWriter, request *dns.Msg) {
		response := new(dns.Msg)
		response.SetReply(request)
		response.Answer = append(response.Answer, &dns.A{Hdr: dns.RR_Header{Name: request.Question[0].Name, Rrtype: dns.TypeA, Class: dns.ClassINET, Ttl: 60}, A: net.ParseIP("203.0.113.9")})
		_ = w.WriteMsg(response)
	})
	var r measureResolvers
	tcpListener, _ := net.Listen("tcp", "127.0.0.1:0")
	r.tcp = uint16(tcpListener.Addr().(*net.TCPAddr).Port)
	tcpServer := &dns.Server{Listener: tcpListener, Handler: handler}
	go func() { _ = tcpServer.ActivateAndServe() }()
	udpConn, _ := net.ListenPacket("udp", "127.0.0.1:0")
	r.udp = uint16(udpConn.LocalAddr().(*net.UDPAddr).Port)
	udpServer := &dns.Server{PacketConn: udpConn, Handler: handler}
	go func() { _ = udpServer.ActivateAndServe() }()

	https := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		body, _ := io.ReadAll(req.Body)
		var request dns.Msg
		if err := request.Unpack(body); err != nil {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		response := new(dns.Msg)
		response.SetReply(&request)
		response.Answer = append(response.Answer, &dns.A{Hdr: dns.RR_Header{Name: request.Question[0].Name, Rrtype: dns.TypeA, Class: dns.ClassINET, Ttl: 60}, A: net.ParseIP("203.0.113.9")})
		packed, _ := response.Pack()
		w.Header().Set("Content-Type", "application/dns-message")
		_, _ = w.Write(packed)
	}))
	https.EnableHTTP2 = true
	https.StartTLS()
	r.https = uint16(https.Listener.Addr().(*net.TCPAddr).Port)

	tlsListener, _ := tls.Listen("tcp", "127.0.0.1:0", https.TLS.Clone())
	r.tls = uint16(tlsListener.Addr().(*net.TCPAddr).Port)
	tlsServer := &dns.Server{Listener: tlsListener, Net: "tcp-tls", Handler: handler}
	go func() { _ = tlsServer.ActivateAndServe() }()
	t.Cleanup(func() { _ = tcpServer.Shutdown(); _ = udpServer.Shutdown(); _ = tlsServer.Shutdown(); https.Close() })
	return r
}

func startShadowsocksServerUDP(t *testing.T) uint16 {
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

// startDelayRelay forwards TCP and UDP on one port to target, delaying every
// chunk by oneWay in each direction (order preserved).
func startDelayRelay(t *testing.T, target uint16, oneWay time.Duration) uint16 {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := uint16(listener.Addr().(*net.TCPAddr).Port)
	targetAddr := net.JoinHostPort("127.0.0.1", strconv.Itoa(int(target)))
	delayedCopy := func(dst io.Writer, src io.Reader) {
		type chunk struct {
			at   time.Time
			data []byte
		}
		queue := make(chan chunk, 1024)
		go func() {
			for c := range queue {
				time.Sleep(time.Until(c.at))
				if _, err := dst.Write(c.data); err != nil {
					return
				}
			}
			if closer, ok := dst.(interface{ CloseWrite() error }); ok {
				_ = closer.CloseWrite()
			}
		}()
		buffer := make([]byte, 32*1024)
		for {
			n, err := src.Read(buffer)
			if n > 0 {
				queue <- chunk{at: time.Now().Add(oneWay), data: append([]byte(nil), buffer[:n]...)}
			}
			if err != nil {
				close(queue)
				return
			}
		}
	}
	go func() {
		for {
			client, err := listener.Accept()
			if err != nil {
				return
			}
			go func() {
				// The TCP handshake itself costs one round trip.
				time.Sleep(2 * oneWay)
				server, err := net.Dial("tcp", targetAddr)
				if err != nil {
					client.Close()
					return
				}
				go delayedCopy(server, client)
				delayedCopy(client, server)
			}()
		}
	}()
	udpConn, err := net.ListenPacket("udp", net.JoinHostPort("127.0.0.1", strconv.Itoa(int(port))))
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		var mu sync.Mutex
		upstreams := map[string]net.Conn{}
		buffer := make([]byte, 65535)
		for {
			n, from, err := udpConn.ReadFrom(buffer)
			if err != nil {
				return
			}
			data := append([]byte(nil), buffer[:n]...)
			mu.Lock()
			upstream, ok := upstreams[from.String()]
			if !ok {
				upstream, _ = net.Dial("udp", targetAddr)
				upstreams[from.String()] = upstream
				go func(from net.Addr) {
					reply := make([]byte, 65535)
					for {
						n, err := upstream.Read(reply)
						if err != nil {
							return
						}
						data := append([]byte(nil), reply[:n]...)
						time.AfterFunc(oneWay, func() { _, _ = udpConn.WriteTo(data, from) })
					}
				}(from)
			}
			mu.Unlock()
			time.AfterFunc(oneWay, func() { _, _ = upstream.Write(data) })
		}
	}()
	t.Cleanup(func() { listener.Close(); udpConn.Close() })
	return port
}

// startMeasureCore builds the real TUN configuration, swaps the TUN inbound
// for a SOCKS listener tagged "tun" and points dns-remote at the local
// resolver over the given transport, still through the selected node.
func startMeasureCore(t *testing.T, nodePort uint16, transport string, r measureResolvers) (uint16, context.Context, func()) {
	t.Helper()
	ingress := localSSIngress(profile.IngressRolePrimary, "n0", 0, nodePort)
	ingress.Capabilities.UDP = true
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "measure", ExpiresAt: time.Now().Add(time.Hour),
		Nodes:     []profile.Node{{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true, UDP: true}, Ingresses: []profile.Ingress{ingress}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	built, err := config.Build(p, profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}, LogLevel: "error"}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	options := built.Options
	socksPort := freePort(t)
	listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	options.Inbounds = []option.Inbound{{Type: C.TypeSOCKS, Tag: config.TUNInboundTag, Options: &option.SocksInboundOptions{ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: socksPort}}}}
	options.Route.AutoDetectInterface = false
	remote := option.RemoteDNSServerOptions{
		RawLocalDNSServerOptions: option.RawLocalDNSServerOptions{DialerOptions: option.DialerOptions{Detour: "selected"}},
		DNSServerAddressOptions:  option.DNSServerAddressOptions{Server: "127.0.0.1"},
	}
	insecure := option.OutboundTLSOptionsContainer{TLS: &option.OutboundTLSOptions{Enabled: true, Insecure: true}}
	var server option.DNSServerOptions
	switch transport {
	case "tcp":
		remote.ServerPort = r.tcp
		server = option.DNSServerOptions{Type: C.DNSTypeTCP, Options: &remote}
	case "udp":
		remote.ServerPort = r.udp
		server = option.DNSServerOptions{Type: C.DNSTypeUDP, Options: &remote}
	case "tls":
		remote.ServerPort = r.tls
		server = option.DNSServerOptions{Type: C.DNSTypeTLS, Options: &option.RemoteTLSDNSServerOptions{RemoteDNSServerOptions: remote, OutboundTLSOptionsContainer: insecure}}
	case "https":
		remote.ServerPort = r.https
		server = option.DNSServerOptions{Type: C.DNSTypeHTTPS, Options: &option.RemoteHTTPSDNSServerOptions{RemoteTLSDNSServerOptions: option.RemoteTLSDNSServerOptions{RemoteDNSServerOptions: remote, OutboundTLSOptionsContainer: insecure}}}
	}
	server.Tag = config.DNSRemoteTag
	for i := range options.DNS.Servers {
		if options.DNS.Servers[i].Tag == config.DNSRemoteTag {
			options.DNS.Servers[i] = server
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	ctx = failover.Context(ctx)
	instance, err := box.New(box.Options{Context: ctx, Options: options})
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	if err = instance.Start(); err != nil {
		cancel()
		t.Fatal(err)
	}
	return socksPort, ctx, func() { instance.Close(); cancel() }
}

// hijackedQuery sends one A query over TCP to a port-53 address through the
// "tun" inbound and returns how long the answer took.
func hijackedQuery(t *testing.T, socksPort uint16, name string) time.Duration {
	t.Helper()
	started := time.Now()
	conn := socksConnect(t, socksPort, netip.MustParseAddrPort("10.255.0.1:53"))
	defer conn.Close()
	query := new(dns.Msg)
	query.SetQuestion(name, dns.TypeA)
	packed, _ := query.Pack()
	if _, err := conn.Write(append(binary.BigEndian.AppendUint16(nil, uint16(len(packed))), packed...)); err != nil {
		t.Fatal(err)
	}
	var length uint16
	if err := binary.Read(conn, binary.BigEndian, &length); err != nil {
		t.Fatalf("%s: %v", name, err)
	}
	if _, err := io.ReadFull(conn, make([]byte, length)); err != nil {
		t.Fatal(err)
	}
	return time.Since(started)
}
