package localdns

import (
	"context"
	"encoding/binary"
	"io"
	"net"
	"net/netip"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/dns"
	"github.com/sagernet/sing-box/dns/transport/hosts"
	"github.com/sagernet/sing/common/control"
	N "github.com/sagernet/sing/common/network"

	mDNS "github.com/miekg/dns"
)

// testServer answers A queries on UDP and TCP of one loopback port. With
// truncate, UDP answers carry only the TC bit; with stray, a mismatched
// datagram comes first.
type testServer struct {
	addr     netip.AddrPort
	answer   netip.Addr
	truncate bool
	stray    bool
	tcp      chan struct{}
}

// listenUDPAndTCP opens a UDP and a TCP listener on the same loopback port
// (a truncated answer is asked again over TCP at the same address). The
// port the UDP bind got may be taken for TCP by another socket: then both
// are dropped and another port tried.
func listenUDPAndTCP(t *testing.T) (net.PacketConn, net.Listener, int) {
	t.Helper()
	var lastErr error
	for range 20 {
		packet, err := net.ListenPacket("udp", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		port := packet.LocalAddr().(*net.UDPAddr).Port
		listener, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
		if err == nil {
			return packet, listener, port
		}
		packet.Close()
		lastErr = err
	}
	t.Fatalf("no loopback port free for both UDP and TCP: %v", lastErr)
	return nil, nil, 0
}

func startServer(t *testing.T, answer string, truncate, stray bool) *testServer {
	t.Helper()
	packet, listener, port := listenUDPAndTCP(t)
	t.Cleanup(func() { packet.Close(); listener.Close() })
	s := &testServer{addr: netip.AddrPortFrom(netip.MustParseAddr("127.0.0.1"), uint16(port)), answer: netip.MustParseAddr(answer), truncate: truncate, stray: stray, tcp: make(chan struct{}, 4)}
	go func() {
		buffer := make([]byte, 65535)
		for {
			n, from, err := packet.ReadFrom(buffer)
			if err != nil {
				return
			}
			query := new(mDNS.Msg)
			if query.Unpack(buffer[:n]) != nil {
				continue
			}
			if s.stray {
				other := s.reply(query)
				other.Id++
				data, _ := other.Pack()
				_, _ = packet.WriteTo(data, from)
			}
			response := s.reply(query)
			if s.truncate {
				response.Answer, response.Truncated = nil, true
			}
			data, _ := response.Pack()
			_, _ = packet.WriteTo(data, from)
		}
	}()
	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			s.tcp <- struct{}{}
			var length [2]byte
			if _, err := io.ReadFull(conn, length[:]); err != nil {
				conn.Close()
				continue
			}
			data := make([]byte, binary.BigEndian.Uint16(length[:]))
			if _, err := io.ReadFull(conn, data); err != nil {
				conn.Close()
				continue
			}
			query := new(mDNS.Msg)
			_ = query.Unpack(data)
			packed, _ := s.reply(query).Pack()
			_, _ = conn.Write(append(binary.BigEndian.AppendUint16(nil, uint16(len(packed))), packed...))
			conn.Close()
		}
	}()
	return s
}

func (s *testServer) reply(query *mDNS.Msg) *mDNS.Msg {
	response := new(mDNS.Msg)
	response.SetReply(query)
	response.Answer = []mDNS.RR{&mDNS.A{Hdr: mDNS.RR_Header{Name: query.Question[0].Name, Rrtype: mDNS.TypeA, Class: mDNS.ClassINET, Ttl: 60}, A: s.answer.AsSlice()}}
	return response
}

// testTransport is a transport over the system dialer (no sing-box context).
func testTransport(override []netip.AddrPort, c *cache) *Transport {
	t := &Transport{
		TransportAdapter: dns.NewTransportAdapter(Type, "dns-local", nil),
		log:              corelog.New(nil),
		hosts:            hosts.NewFile("/nonexistent"),
		dialer:           N.SystemDialer,
		override:         override,
		cache:            c,
	}
	if c == nil {
		t.cache = &cache{current: func() *control.Interface { return nil }, now: time.Now}
	}
	return t
}

func query(name string) *mDNS.Msg {
	message := new(mDNS.Msg)
	message.SetQuestion(name, mDNS.TypeA)
	message.RecursionDesired = true
	return message
}

func answerOf(t *testing.T, response *mDNS.Msg) string {
	t.Helper()
	if len(response.Answer) != 1 {
		t.Fatalf("answer: %v", response)
	}
	return response.Answer[0].(*mDNS.A).A.String()
}

func TestExchangeAsksServersInOrder(t *testing.T) {
	stray := startServer(t, "192.0.2.1", false, true)
	truncated := startServer(t, "192.0.2.2", true, false)

	// A mismatched datagram is skipped, the real answer taken.
	ctx, upstream := WithUpstream(context.Background())
	response, err := testTransport([]netip.AddrPort{stray.addr}, nil).Exchange(ctx, query("a.lab.test."))
	if err != nil || answerOf(t, response) != "192.0.2.1" || upstream() != stray.addr.String() {
		t.Fatalf("stray: %v %v %q", response, err, upstream())
	}

	// A truncated UDP answer is asked again over TCP.
	response, err = testTransport([]netip.AddrPort{truncated.addr}, nil).Exchange(context.Background(), query("b.lab.test."))
	if err != nil || answerOf(t, response) != "192.0.2.2" || len(truncated.tcp) != 1 {
		t.Fatalf("truncated: %v %v, %d tcp", response, err, len(truncated.tcp))
	}

	// A server that does not answer (port 9, discard: nothing listens, or
	// nothing replies) is skipped for the next one.
	unanswered := netip.AddrPortFrom(netip.MustParseAddr("127.0.0.1"), 9)
	started := time.Now()
	ctx, upstream = WithUpstream(context.Background())
	response, err = testTransport([]netip.AddrPort{unanswered, stray.addr}, nil).Exchange(ctx, query("c.lab.test."))
	if err != nil || answerOf(t, response) != "192.0.2.1" || upstream() != stray.addr.String() {
		t.Fatalf("fallthrough: %v %v %q", response, err, upstream())
	}
	if elapsed := time.Since(started); elapsed > ServerTimeout+time.Second {
		t.Fatalf("fallthrough took %v", elapsed)
	}
}

// Every server failing marks the read servers suspect, so the next query
// reads the interface again (after RetryInterval).
func TestExchangeFailureRereads(t *testing.T) {
	source := &fakeSource{servers: map[string][]netip.AddrPort{}}
	current := &control.Interface{Index: 6, Name: "en0"}
	now := time.Unix(1000, 0)
	c, _ := newTestCache(source, &current, &now)
	c.exclude = nil
	// Loopback is never usable from the interface; feed the cache directly.
	c.read, c.ifIndex, c.readAt, c.triedAt = true, 6, now, now
	c.servers = []netip.AddrPort{netip.AddrPortFrom(netip.MustParseAddr("127.0.0.1"), 9)}
	transport := testTransport(nil, c)
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if response, err := transport.Exchange(ctx, query("d.lab.test.")); err != nil || response.Rcode != mDNS.RcodeServerFailure {
		t.Fatalf("every server failed: %v %v", response, err)
	}
	if !c.stale {
		t.Fatal("not marked stale")
	}
	source.set("en0", "192.168.1.1")
	now = now.Add(RetryInterval)
	if servers, err := c.get(context.Background()); err != nil || joinServers(servers) != "192.168.1.1:53" {
		t.Fatalf("reread: %v %v", servers, err)
	}
}

// Without servers a hijacked query gets SERVFAIL at once (an error would
// leave the client waiting for its own timeout), with the cause logged.
func TestExchangeWithoutServersAnswersServfailAtOnce(t *testing.T) {
	var logs strings.Builder
	transport := testTransport(nil, nil)
	transport.log = corelog.New(&logs)
	if err := transport.log.SetLevel("debug"); err != nil {
		t.Fatal(err)
	}
	started := time.Now()
	message := query("e.lab.test.")
	response, err := transport.Exchange(context.Background(), message)
	if err != nil || response.Rcode != mDNS.RcodeServerFailure || response.Id != message.Id || time.Since(started) > 100*time.Millisecond {
		t.Fatalf("%v %v after %v", response, err, time.Since(started))
	}
	if !strings.Contains(logs.String(), `msg="local dns failed" name=e.lab.test. error="local dns: no default interface"`) {
		t.Fatalf("log: %s", logs.String())
	}
}
