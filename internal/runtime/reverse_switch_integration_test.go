package runtime

import (
	"context"
	"encoding/binary"
	"io"
	"net/netip"
	"testing"
	"time"

	"github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/reversemap"
	"github.com/peakpassvpn/ppvpn-core/profile"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

// A kernel switch keeps the reverse mapping: a client that resolved a name
// through the old kernel and connects to the address through the new one
// still matches the name's route rule and the node gets the name. (The new
// kernel's own dns.reverse_mapping starts empty.)
func TestKernelSwitchKeepsReverseMapping(t *testing.T) {
	nodeA, nodeB := &destinationRecorder{}, &destinationRecorder{}
	dnsPort := startFakeDNS(t, map[string]string{"b.test.": "203.0.113.8"})
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "reverse", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{
			{ID: "a", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "a0", 0, startShadowsocksServerWithTracker(t, nodeA))}},
			{ID: "b", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "b0", 0, startShadowsocksServerWithTracker(t, nodeB))}},
		},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "a"},
		Routing: profile.Routing{
			Rules: []profile.RoutingRule{{ID: "b-by-name", Match: profile.RoutingMatch{Domains: []string{"b.test"}}, Action: profile.RoutingAction{Type: "proxy", Target: "node", NodeID: "b"}}},
			Final: profile.RoutingAction{Type: "proxy", Target: "selected"},
		},
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
	for i, server := range options.DNS.Servers {
		if server.Tag == config.DNSRemoteTag {
			options.DNS.Servers[i] = option.DNSServerOptions{Type: C.DNSTypeTCP, Tag: config.DNSRemoteTag, Options: &option.RemoteDNSServerOptions{
				RawLocalDNSServerOptions: option.RawLocalDNSServerOptions{DialerOptions: option.DialerOptions{Detour: "selected"}},
				DNSServerAddressOptions:  option.DNSServerAddressOptions{Server: "127.0.0.1", ServerPort: dnsPort},
			}}
		}
	}
	ctx := reversemap.WithStore(context.Background(), reversemap.New())
	engine, err := newLayeredEngine(ctx, options)
	if err != nil {
		t.Fatal(err)
	}
	if err = engine.Start(); err != nil {
		t.Fatal(err)
	}
	defer engine.Close()

	// Resolve b.test through the hijacked DNS of kernel 1.
	conn := socksConnect(t, socksPort, netip.MustParseAddrPort("10.255.0.1:53"))
	query := new(dns.Msg)
	query.SetQuestion("b.test.", dns.TypeA)
	packed, _ := query.Pack()
	if _, err = conn.Write(append(binary.BigEndian.AppendUint16(nil, uint16(len(packed))), packed...)); err != nil {
		t.Fatal(err)
	}
	var length uint16
	if err = binary.Read(conn, binary.BigEndian, &length); err != nil {
		t.Fatal(err)
	}
	if _, err = io.ReadFull(conn, make([]byte, length)); err != nil {
		t.Fatal(err)
	}
	conn.Close()

	if _, err = engine.swap(ctx, options, nil, nil); err != nil {
		t.Fatal(err)
	}
	// Connect to the address through kernel 2 with bytes no sniffer
	// recognises: the domain can only come from the reverse mapping. (An HTTP
	// request with the address as Host would not do: the HTTP sniffer
	// replaces the domain with the address, with or without a switch.)
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("203.0.113.8:9999"))
	defer conn.Close()
	_, _ = conn.Write([]byte("ping\n"))
	nodeB.wait(t, "b.test:9999")
	if nodeA.has("203.0.113.8:9999") || nodeA.has("b.test:9999") {
		t.Fatal("connection went to the selected node: the b.test rule did not match after the switch")
	}
}
