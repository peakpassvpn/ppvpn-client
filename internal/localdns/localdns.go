// Package localdns is the core's direct resolver ("dns-local") on Windows
// and macOS: it asks the DNS servers of the physical default interface (the
// one auto_detect_interface binds direct sockets to), read again on every
// default interface change.
//
// It replaces sing-box's local transport there, which caches the system DNS
// configuration for up to 5 s without regard to interface changes (Windows)
// or falls back to the system resolver the desktop points at the tunnel
// (Darwin), and the static --local-dns-servers list that never followed a
// network change. It never asks the system resolver and never falls back to
// 127.0.0.1: without servers a query fails at once.
//
// See docs/design-local-dns.md.
package localdns

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net/netip"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/dns"
	"github.com/sagernet/sing-box/dns/transport/hosts"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing-tun"
	"github.com/sagernet/sing/common/control"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/common/x/list"
	"github.com/sagernet/sing/service"

	mDNS "github.com/miekg/dns"
)

// Type is the DNS server type of this transport.
const Type = "ppvpn-local"

// ServerTimeout bounds one exchange with one server.
const ServerTimeout = 2 * time.Second

// Options configures the transport.
type Options struct {
	// Servers, when set, are used as they are (the host's
	// --local-dns-servers): the default interface is not read.
	Servers []netip.AddrPort `json:"servers,omitempty"`
	// Exclude lists prefixes a resolver read from the interface must not be
	// in: the core's own tunnel ranges, whose resolver loops back.
	Exclude []netip.Prefix `json:"exclude,omitempty"`
}

// Supported reports whether this build reads the default interface's
// resolvers: on Windows and macOS, and in builds with the
// localdns_testsource tag (lab tests).
func Supported() bool {
	return runtime.GOOS == "windows" || runtime.GOOS == "darwin" || testSource
}

// Register adds the transport to registry, constructed by newTransport
// (dnstransport wraps it for logging and the reverse mapping).
func Register(registry *dns.TransportRegistry, constructor dns.TransportConstructorFunc[Options]) {
	dns.RegisterTransport[Options](registry, Type, constructor)
}

var _ adapter.DNSTransport = (*Transport)(nil)

// Transport implements adapter.DNSTransport.
type Transport struct {
	dns.TransportAdapter
	ctx      context.Context
	log      *corelog.Logger
	hosts    *hosts.File
	dialer   N.Dialer
	override []netip.AddrPort
	cache    *cache

	mu       sync.Mutex
	monitor  tun.DefaultInterfaceMonitor
	callback *list.Element[tun.DefaultInterfaceUpdateCallback]
}

// NewTransport creates the transport; l receives the "local dns servers"
// lines (nil discards them).
func NewTransport(ctx context.Context, l *corelog.Logger, tag string, options Options) (*Transport, error) {
	transportDialer, err := dns.NewLocalDialer(ctx, option.LocalDNSServerOptions{})
	if err != nil {
		return nil, err
	}
	t := &Transport{
		TransportAdapter: dns.NewTransportAdapter(Type, tag, nil),
		ctx:              ctx,
		log:              l,
		hosts:            hosts.NewFile(hosts.DefaultPath),
		dialer:           transportDialer,
		override:         options.Servers,
	}
	t.cache = &cache{
		discover: discover,
		current:  t.defaultInterface,
		exclude:  options.Exclude,
		now:      time.Now,
		changed:  t.logServers,
	}
	return t, nil
}

func (t *Transport) Start(stage adapter.StartStage) error {
	if stage != adapter.StartStateStart {
		return nil
	}
	if len(t.override) > 0 {
		t.log.Info("local dns servers", "source", "override", "interface", "-", "servers", joinServers(t.override))
		return nil
	}
	manager := service.FromContext[adapter.NetworkManager](t.ctx)
	if manager == nil || manager.InterfaceMonitor() == nil {
		// Without auto_detect_interface there is no default interface to
		// read: every query fails with ErrNoInterface.
		t.log.Warn("local dns servers", "source", "none", "interface", "-", "servers", "none", "error", "no interface monitor (auto_detect_interface is off)")
		return nil
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	t.monitor = manager.InterfaceMonitor()
	t.callback = t.monitor.RegisterCallback(func(*control.Interface, int) { t.cache.invalidate() })
	return nil
}

func (t *Transport) Close() error {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.callback != nil {
		t.monitor.UnregisterCallback(t.callback)
		t.callback = nil
	}
	return nil
}

// Reset reads the resolvers again on the next query.
func (t *Transport) Reset() { t.cache.invalidate() }

func (t *Transport) defaultInterface() *control.Interface {
	t.mu.Lock()
	monitor := t.monitor
	t.mu.Unlock()
	if monitor == nil {
		return nil
	}
	return monitor.DefaultInterface()
}

func (t *Transport) logServers(iface control.Interface, source string, servers []netip.AddrPort, err error) {
	if err != nil {
		t.log.Warn("local dns servers", "source", source, "interface", iface.Name, "servers", "none", "error", err)
		return
	}
	t.log.Info("local dns servers", "source", source, "interface", iface.Name, "servers", joinServers(servers))
}

func (t *Transport) Exchange(ctx context.Context, message *mDNS.Msg) (*mDNS.Msg, error) {
	question := message.Question[0]
	if question.Qtype == mDNS.TypeA || question.Qtype == mDNS.TypeAAAA {
		if addresses := t.hosts.Lookup(dns.FqdnToDomain(question.Name)); len(addresses) > 0 {
			return dns.FixedResponse(message.Id, question, addresses, C.DefaultDNSTTL), nil
		}
	}
	servers := t.override
	if len(servers) == 0 {
		var err error
		if servers, err = t.cache.get(ctx); err != nil {
			return t.serverFailure(message, err), nil
		}
	}
	var errs []error
	for _, server := range servers {
		response, err := t.exchangeOne(ctx, server, message)
		if err == nil {
			setUpstream(ctx, server)
			return response, nil
		}
		errs = append(errs, fmt.Errorf("%s: %w", server, err))
		if ctx.Err() != nil {
			return nil, errors.Join(errs...)
		}
	}
	if len(t.override) == 0 {
		t.cache.failed()
	}
	return t.serverFailure(message, errors.Join(errs...)), nil
}

// serverFailure answers SERVFAIL for a query no server could answer (none
// read, or every one failed). sing-box answers a hijacked query whose
// exchange errors with nothing (UDP) or a closed connection (TCP), so the
// client would wait out its own timeout; SERVFAIL fails it now, and is never
// cached. The cause goes to the debug log ("local dns servers" says why
// there are none).
func (t *Transport) serverFailure(message *mDNS.Msg, cause error) *mDNS.Msg {
	if t.log.DebugEnabled() {
		t.log.Debug("local dns failed", "name", message.Question[0].Name, "error", cause)
	}
	response := new(mDNS.Msg)
	response.SetRcode(message, mDNS.RcodeServerFailure)
	return response
}

// exchangeOne asks server over UDP, and again over TCP when the answer is
// truncated, within ServerTimeout.
func (t *Transport) exchangeOne(ctx context.Context, server netip.AddrPort, message *mDNS.Msg) (*mDNS.Msg, error) {
	ctx, cancel := context.WithTimeout(ctx, ServerTimeout)
	defer cancel()
	packed, err := message.Pack()
	if err != nil {
		return nil, err
	}
	response, err := t.exchangeUDP(ctx, server, message, packed)
	if err != nil || !response.Truncated {
		return response, err
	}
	return t.exchangeTCP(ctx, server, message, packed)
}

func (t *Transport) exchangeUDP(ctx context.Context, server netip.AddrPort, message *mDNS.Msg, packed []byte) (*mDNS.Msg, error) {
	conn, err := t.dialer.DialContext(ctx, N.NetworkUDP, M.SocksaddrFromNetIP(server))
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	stop := context.AfterFunc(ctx, func() { _ = conn.SetDeadline(time.Unix(1, 0)) })
	defer stop()
	if deadline, ok := ctx.Deadline(); ok {
		_ = conn.SetDeadline(deadline)
	}
	if _, err := conn.Write(packed); err != nil {
		return nil, err
	}
	buffer := make([]byte, 65535)
	for {
		n, err := conn.Read(buffer)
		if err != nil {
			return nil, contextError(ctx, err)
		}
		response := new(mDNS.Msg)
		if response.Unpack(buffer[:n]) != nil || !answers(response, message) {
			// A stray or late datagram: keep waiting for ours.
			continue
		}
		return response, nil
	}
}

func (t *Transport) exchangeTCP(ctx context.Context, server netip.AddrPort, message *mDNS.Msg, packed []byte) (*mDNS.Msg, error) {
	conn, err := t.dialer.DialContext(ctx, N.NetworkTCP, M.SocksaddrFromNetIP(server))
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	stop := context.AfterFunc(ctx, func() { _ = conn.SetDeadline(time.Unix(1, 0)) })
	defer stop()
	if deadline, ok := ctx.Deadline(); ok {
		_ = conn.SetDeadline(deadline)
	}
	framed := binary.BigEndian.AppendUint16(make([]byte, 0, 2+len(packed)), uint16(len(packed)))
	if _, err := conn.Write(append(framed, packed...)); err != nil {
		return nil, contextError(ctx, err)
	}
	var length [2]byte
	if _, err := io.ReadFull(conn, length[:]); err != nil {
		return nil, contextError(ctx, err)
	}
	buffer := make([]byte, binary.BigEndian.Uint16(length[:]))
	if _, err := io.ReadFull(conn, buffer); err != nil {
		return nil, contextError(ctx, err)
	}
	response := new(mDNS.Msg)
	if err := response.Unpack(buffer); err != nil {
		return nil, err
	}
	if !answers(response, message) {
		return nil, errors.New("answer does not match the query")
	}
	return response, nil
}

// answers reports whether response is the reply to message.
func answers(response, message *mDNS.Msg) bool {
	if response.Id != message.Id || !response.Response || len(response.Question) != len(message.Question) {
		return false
	}
	for i, question := range message.Question {
		got := response.Question[i]
		if got.Qtype != question.Qtype || got.Qclass != question.Qclass || !strings.EqualFold(got.Name, question.Name) {
			return false
		}
	}
	return true
}

// contextError reports the context's error for an I/O error caused by its
// deadline or cancellation.
func contextError(ctx context.Context, err error) error {
	if ctxErr := ctx.Err(); ctxErr != nil {
		return ctxErr
	}
	return err
}

type upstreamKey struct{}

// WithUpstream returns a context in which Exchange records the server that
// answered, and a function returning it ("" when none answered).
func WithUpstream(ctx context.Context) (context.Context, func() string) {
	slot := new(netip.AddrPort)
	return context.WithValue(ctx, upstreamKey{}, slot), func() string {
		if !slot.IsValid() {
			return ""
		}
		return slot.String()
	}
}

func setUpstream(ctx context.Context, server netip.AddrPort) {
	if slot, ok := ctx.Value(upstreamKey{}).(*netip.AddrPort); ok {
		*slot = server
	}
}
