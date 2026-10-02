package runtime

import (
	"bytes"
	"context"
	"crypto/tls"
	"encoding/binary"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"reflect"
	goruntime "runtime"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/internal/goldentest"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/routing"
	box "github.com/sagernet/sing-box"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

// The routing golden files (testdata/golden/routing) record, for a profile
// and a set of connections, what the real engine decides: the action, the
// node, and the destination the outbound is given. They are the baseline
// the Rust engine's routing is checked against. Connections are fed into
// sing-box the way the products do (the TUN through a SOCKS inbound carrying
// the TUN tag, with TLS or HTTP bytes to sniff; the local proxy and the
// system proxy as themselves); every outbound is bound to the loopback
// interface, so dials fail at once and nothing leaves the host. The
// decision is read from the connection log, written when routing chose an
// outbound. go test -run TestGoldenRouting -update rewrites "expect".
var updateRoutingGolden = flag.Bool("update", false, "rewrite the golden files' expectations from the current behaviour")

const routingGoldenDir = "../../testdata/golden/routing"

// The fixed time the profile is built at; its validity is checked against it.
var routingGoldenNow = time.Date(2026, 10, 1, 0, 0, 0, 0, time.UTC)

type routingFile struct {
	Name        string                `json:"name"`
	Description string                `json:"description"`
	ProfileRef  goldentest.ProfileRef `json:"profile_ref"`
	Cases       []routingCase         `json:"cases"`
	Expect      []json.RawMessage     `json:"expect,omitempty"`
}

// routingCase is one connection. Inbound is "tun", "proxy-routed" (the
// local proxy's routed user), "proxy-node:<id>" (that node's user) or
// "system-proxy". Destination is host:port as the inbound receives it (an
// IP for the TUN). RejectReason only annotates an expected REJECT for
// readers and the Rust engine; the log cannot tell rejections apart.
type routingCase struct {
	Name          string        `json:"name"`
	Inbound       string        `json:"inbound"`
	RoutingMode   string        `json:"routing_mode,omitempty"`
	HostIPv6Route *bool         `json:"host_ipv6_route,omitempty"`
	Destination   string        `json:"destination"`
	Sniff         *routingSniff `json:"sniff,omitempty"`
	RejectReason  string        `json:"reject_reason,omitempty"`
}

type routingSniff struct {
	TLSServerName string `json:"tls_server_name,omitempty"`
	HTTPHost      string `json:"http_host,omitempty"`
}

// routingResult is the engine's decision. Target is the destination the
// chosen outbound is asked for (a sniffed domain handed to a node, or the
// IP), IPv6HandOff marks direct traffic to a global IPv6 address handed to
// its domain over IPv4 (host without IPv6). Classifier is what the flow
// adapter's routing.Classifier decides for the same flow, for comparison:
// it has no client floor, fake-ip or TUN-subnet rules and no rule sets.
type routingResult struct {
	Action      string            `json:"action"`
	NodeID      string            `json:"node_id,omitempty"`
	Target      string            `json:"target,omitempty"`
	TargetKind  string            `json:"target_kind,omitempty"`
	IPv6HandOff bool              `json:"ipv6_hand_off,omitempty"`
	Classifier  *routing.Decision `json:"classifier,omitempty"`
}

func TestGoldenRouting(t *testing.T) {
	files, err := filepath.Glob(filepath.Join(routingGoldenDir, "*.json"))
	if err != nil || len(files) == 0 {
		t.Fatalf("no routing files in %s: %v", routingGoldenDir, err)
	}
	for _, path := range files {
		t.Run(strings.TrimSuffix(filepath.Base(path), ".json"), func(t *testing.T) {
			raw, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			var file routingFile
			if err := json.Unmarshal(raw, &file); err != nil {
				t.Fatal(err)
			}
			results := runRoutingFile(t, file)
			if *updateRoutingGolden {
				file.Expect = results
				var buf bytes.Buffer
				encoder := json.NewEncoder(&buf)
				encoder.SetEscapeHTML(false)
				encoder.SetIndent("", "  ")
				if err := encoder.Encode(file); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(path, buf.Bytes(), 0o644); err != nil {
					t.Fatal(err)
				}
				return
			}
			if len(file.Expect) != len(results) {
				t.Fatalf("%d expectations for %d cases; rerun with -update", len(file.Expect), len(results))
			}
			for i := range results {
				var want, got any
				_ = json.Unmarshal(file.Expect[i], &want)
				_ = json.Unmarshal(results[i], &got)
				if !reflect.DeepEqual(want, got) {
					t.Errorf("%s:\n got %s\nwant %s", file.Cases[i].Name, results[i], file.Expect[i])
				}
			}
		})
	}
}

// routingInstance is one engine: a TUN-only build or a local-proxy build,
// for one routing mode and host IPv6 state, as the desktop runs them.
type routingInstance struct {
	tun        bool
	mode       RoutingMode
	hostIPv6   bool
	socksPort  uint16
	proxyPort  uint16
	systemPort uint16
	prefix     string
	password   string
	built      *config.BuildResult
	effective  *profile.Profile
	log        *routingLog
}

func runRoutingFile(t *testing.T, file routingFile) []json.RawMessage {
	t.Helper()
	profileJSON, err := goldentest.ResolveProfile(routingGoldenDir, file.ProfileRef)
	if err != nil {
		t.Fatal(err)
	}
	instances := map[string]*routingInstance{}
	results := make([]json.RawMessage, 0, len(file.Cases))
	for _, c := range file.Cases {
		mode, err := ParseRoutingMode(c.RoutingMode)
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		hostIPv6 := c.HostIPv6Route == nil || *c.HostIPv6Route
		tun := c.Inbound == "tun"
		key := fmt.Sprintf("%v/%s/%v", tun, mode, hostIPv6)
		instance := instances[key]
		if instance == nil {
			p, err := profile.Parse(profileJSON)
			if err != nil {
				t.Fatal(err)
			}
			instance = startRoutingInstance(t, p, tun, mode, hostIPv6)
			instances[key] = instance
		}
		result := instance.decide(t, c)
		out, err := json.Marshal(result)
		if err != nil {
			t.Fatal(err)
		}
		results = append(results, out)
	}
	return results
}

func startRoutingInstance(t *testing.T, p *profile.Profile, tun bool, mode RoutingMode, hostIPv6 bool) *routingInstance {
	t.Helper()
	effective, err := effectiveProfile(p, mode)
	if err != nil {
		t.Fatal(err)
	}
	instance := &routingInstance{tun: tun, mode: mode, hostIPv6: hostIPv6, effective: effective, prefix: "gold0", password: "golden-password"}
	var built *config.BuildResult
	if tun {
		platform := profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}, LogLevel: "error"}
		built, err = config.BuildWithOptions(effective, platform, config.BuildOptions{NoHostIPv6Route: !hostIPv6}, routingGoldenNow)
		if err != nil {
			t.Fatal(err)
		}
		instance.socksPort = freePort(t)
		listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
		for i, inbound := range built.Options.Inbounds {
			if inbound.Tag == config.TUNInboundTag {
				built.Options.Inbounds[i] = option.Inbound{Type: C.TypeSOCKS, Tag: config.TUNInboundTag, Options: &option.SocksInboundOptions{ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: instance.socksPort}}}
			}
		}
	} else {
		platform := profile.PlatformCapabilities{Platform: "linux", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
		instance.proxyPort, instance.systemPort = freePort(t), freePort(t)
		var endpoints []localproxy.Endpoint
		for _, node := range effective.Nodes {
			endpoints = append(endpoints, localproxy.Endpoint{NodeID: node.ID, Listen: "127.0.0.1", Port: instance.proxyPort,
				Username: localproxy.FormatUsername(instance.prefix, node.ID), Password: instance.password})
		}
		built, err = config.BuildWithOptions(effective, platform, config.BuildOptions{LocalProxies: endpoints}, routingGoldenNow)
		if err != nil {
			t.Fatal(err)
		}
		built = config.WithSystemProxy(built, instance.systemPort)
	}
	instance.built = built
	options := built.Options
	if options.Route != nil {
		options.Route.AutoDetectInterface = false
	}
	bindLoopback(options.Outbounds)
	instance.log = &routingLog{}
	logger := corelog.New(instance.log)
	_ = logger.SetLevel(corelog.LevelDebug)
	ctx, cancel := context.WithCancel(context.Background())
	instanceBox, err := box.New(box.Options{Context: failover.Context(ctx), Options: options})
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	tracker := newTelemetry()
	tracker.log.Store(logger)
	instanceBox.Router().AppendTracker(tracker)
	if err := instanceBox.Start(); err != nil {
		cancel()
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = instanceBox.Close(); cancel() })
	return instance
}

// bindLoopback binds every outbound that dials to the loopback interface:
// a dial to anywhere else fails at once, after routing has decided.
func bindLoopback(outbounds []option.Outbound) {
	name := "lo"
	if goruntime.GOOS == "darwin" {
		name = "lo0"
	}
	for _, outbound := range outbounds {
		value := reflect.ValueOf(outbound.Options)
		if value.Kind() != reflect.Pointer || value.IsNil() {
			continue
		}
		field := value.Elem().FieldByName("DialerOptions")
		if field.IsValid() && field.CanSet() {
			dialer := field.Addr().Interface().(*option.DialerOptions)
			dialer.BindInterface = name
		}
	}
}

func (instance *routingInstance) decide(t *testing.T, c routingCase) routingResult {
	t.Helper()
	_, portText, err := net.SplitHostPort(c.Destination)
	if err != nil {
		t.Fatalf("%s: destination: %v", c.Name, err)
	}
	conn, err := instance.open(c)
	if err != nil {
		t.Fatalf("%s: %v", c.Name, err)
	}
	line := instance.log.wait(":"+portText, conn)
	conn.Close()
	result := routingResult{Action: "REJECT"}
	if line != nil {
		result = instance.interpret(line)
	}
	result.Classifier = instance.classify(c)
	return result
}

// open connects through the case's inbound and sends the bytes to sniff.
func (instance *routingInstance) open(c routingCase) (net.Conn, error) {
	var payload []byte
	if c.Sniff != nil {
		switch {
		case c.Sniff.TLSServerName != "":
			payload = clientHello(c.Sniff.TLSServerName)
		case c.Sniff.HTTPHost != "":
			payload = []byte("GET / HTTP/1.1\r\nHost: " + c.Sniff.HTTPHost + "\r\n\r\n")
		}
	}
	switch {
	case c.Inbound == "tun":
		return socksOpen(instance.socksPort, c.Destination, payload)
	case c.Inbound == "system-proxy":
		return connectOpen(instance.systemPort, "", c.Destination, payload)
	case c.Inbound == "proxy-routed":
		return connectOpen(instance.proxyPort, localproxy.FormatUsername(instance.prefix, "")+":"+instance.password, c.Destination, payload)
	case strings.HasPrefix(c.Inbound, "proxy-node:"):
		node := strings.TrimPrefix(c.Inbound, "proxy-node:")
		return connectOpen(instance.proxyPort, localproxy.FormatUsername(instance.prefix, node)+":"+instance.password, c.Destination, payload)
	}
	return nil, fmt.Errorf("unknown inbound %q", c.Inbound)
}

// socksOpen sends a SOCKS5 greeting, a CONNECT for an IP or a domain and the
// payload at once (the TUN's routing reads the payload to sniff before the
// CONNECT is answered).
func socksOpen(port uint16, destination string, payload []byte) (net.Conn, error) {
	host, portText, _ := net.SplitHostPort(destination)
	destinationPort, _ := strconv.Atoi(portText)
	conn, err := net.DialTimeout("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(int(port))), time.Second)
	if err != nil {
		return nil, err
	}
	request := []byte{5, 1, 0, 5, 1, 0}
	if addr, err := netip.ParseAddr(host); err == nil {
		if addr.Is4() {
			ip := addr.As4()
			request = append(append(request, 1), ip[:]...)
		} else {
			ip := addr.As16()
			request = append(append(request, 4), ip[:]...)
		}
	} else {
		request = append(append(request, 3, byte(len(host))), host...)
	}
	request = binary.BigEndian.AppendUint16(request, uint16(destinationPort))
	if _, err = conn.Write(request); err != nil {
		return conn, err
	}
	// Method reply, then the CONNECT reply (VER REP RSV ATYP ADDR PORT);
	// the payload to sniff goes after it, as an application would send it.
	_ = conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	reply := make([]byte, 2+4)
	if _, err = io.ReadFull(conn, reply); err != nil || reply[3] != 0 {
		return conn, nil // rejected before replying: wait sees the close
	}
	bound := 4 + 2
	if reply[5] == 4 {
		bound = 16 + 2
	}
	if _, err = io.ReadFull(conn, make([]byte, bound)); err != nil {
		return conn, nil
	}
	_ = conn.SetReadDeadline(time.Time{})
	if len(payload) > 0 {
		_, err = conn.Write(payload)
	}
	return conn, err
}

// connectOpen sends an HTTP CONNECT (with Basic credentials "user:password"
// when given) and the payload.
func connectOpen(port uint16, credentials, destination string, payload []byte) (net.Conn, error) {
	conn, err := net.DialTimeout("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(int(port))), time.Second)
	if err != nil {
		return nil, err
	}
	request := "CONNECT " + destination + " HTTP/1.1\r\nHost: " + destination + "\r\n"
	if credentials != "" {
		request += "Proxy-Authorization: Basic " + basicAuth(credentials) + "\r\n"
	}
	_, err = conn.Write(append([]byte(request+"\r\n"), payload...))
	return conn, err
}

func basicAuth(credentials string) string {
	const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
	data := []byte(credentials)
	var out strings.Builder
	for i := 0; i < len(data); i += 3 {
		var chunk [3]byte
		n := copy(chunk[:], data[i:])
		value := uint(chunk[0])<<16 | uint(chunk[1])<<8 | uint(chunk[2])
		for j := 0; j < 4; j++ {
			if j <= n {
				out.WriteByte(alphabet[value>>(18-6*j)&63])
			} else {
				out.WriteByte('=')
			}
		}
	}
	return out.String()
}

// clientHello returns the first TLS record a client sends for serverName.
func clientHello(serverName string) []byte {
	recorder := &helloRecorder{}
	client := tls.Client(recorder, &tls.Config{ServerName: serverName, InsecureSkipVerify: true}) //nolint:gosec // only the ClientHello bytes are used
	_ = client.Handshake()
	return recorder.written.Bytes()
}

type helloRecorder struct{ written bytes.Buffer }

func (r *helloRecorder) Read([]byte) (int, error)         { return 0, errors.New("recorded") }
func (r *helloRecorder) Write(p []byte) (int, error)      { return r.written.Write(p) }
func (r *helloRecorder) Close() error                     { return nil }
func (r *helloRecorder) LocalAddr() net.Addr              { return &net.TCPAddr{} }
func (r *helloRecorder) RemoteAddr() net.Addr             { return &net.TCPAddr{} }
func (r *helloRecorder) SetDeadline(time.Time) error      { return nil }
func (r *helloRecorder) SetReadDeadline(time.Time) error  { return nil }
func (r *helloRecorder) SetWriteDeadline(time.Time) error { return nil }

// interpret maps a connection log line to the decision: direct outbounds
// to DIRECT, the selector to the selected node, node and failover outbounds
// to their node.
func (instance *routingInstance) interpret(fields map[string]string) routingResult {
	result := routingResult{Target: fields["target"], TargetKind: fields["target_kind"]}
	switch tag := fields["outbound"]; {
	case tag == "direct":
		result.Action = "DIRECT"
	case tag == config.DirectHostTag:
		// The physical direct outbound behind the IPv6 hand-off wrapper; it
		// handed off only when it was given a domain instead of the IP.
		result.Action = "DIRECT"
		result.IPv6HandOff = instance.built.DirectIPv6HandOff && result.TargetKind == "domain"
	case tag == "selected":
		result.Action, result.NodeID = "PROXY", instance.effective.Selection.DefaultNodeID
	case instance.built.OutboundNodes[tag] != "":
		result.Action, result.NodeID = "PROXY", instance.built.OutboundNodes[tag]
	default:
		result.Action = "OUTBOUND:" + tag
	}
	return result
}

func (instance *routingInstance) classify(c routingCase) *routing.Decision {
	classifier, err := routing.Compile(instance.effective, routingGoldenNow)
	if err != nil {
		return nil
	}
	host, portText, _ := net.SplitHostPort(c.Destination)
	port, _ := strconv.Atoi(portText)
	flow := routing.Flow{Entry: routing.EntryTransparent, DestinationPort: uint16(port), Protocol: "tcp"}
	if strings.HasPrefix(c.Inbound, "proxy-node:") {
		flow.Entry, flow.LocalProxyNodeID = routing.EntryLocalProxy, strings.TrimPrefix(c.Inbound, "proxy-node:")
	}
	if _, err := netip.ParseAddr(host); err == nil {
		flow.DestinationIP = host
	} else {
		flow.Hostname = host
	}
	if c.Sniff != nil {
		if c.Sniff.TLSServerName != "" {
			flow.Hostname = c.Sniff.TLSServerName
		} else if c.Sniff.HTTPHost != "" {
			flow.Hostname = c.Sniff.HTTPHost
		}
	}
	decision := classifier.Classify(flow, instance.effective.Selection.DefaultNodeID)
	return &routing.Decision{Type: decision.Type, Target: decision.Target, NodeID: decision.NodeID, RuleID: decision.RuleID, Priority: decision.Priority}
}

// routingLog collects the debug log and finds a connection line by port.
type routingLog struct {
	mu   sync.Mutex
	text strings.Builder
}

func (l *routingLog) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.text.Write(p)
}

// wait returns the fields of the connection line whose destination ends in
// suffix, or nil once the connection is closed without one (rejected).
func (l *routingLog) wait(suffix string, conn net.Conn) map[string]string {
	closed := make(chan struct{})
	go func() {
		_ = conn.SetReadDeadline(time.Now().Add(5 * time.Second))
		_, _ = io.Copy(io.Discard, conn)
		close(closed)
	}()
	deadline := time.Now().Add(5 * time.Second)
	var closedAt time.Time
	for time.Now().Before(deadline) {
		if fields := l.find(suffix); fields != nil {
			return fields
		}
		select {
		case <-closed:
			if closedAt.IsZero() {
				closedAt = time.Now()
			}
			// The line is written before the outbound dials, so a closed
			// connection with no line after a grace period was rejected.
			if time.Since(closedAt) > 300*time.Millisecond {
				return nil
			}
		default:
		}
		time.Sleep(10 * time.Millisecond)
	}
	return nil
}

func (l *routingLog) find(suffix string) map[string]string {
	l.mu.Lock()
	text := l.text.String()
	l.mu.Unlock()
	for line := range strings.SplitSeq(text, "\n") {
		if !strings.Contains(line, `msg=connection `) {
			continue
		}
		fields := logfmtFields(line)
		if strings.HasSuffix(fields["destination"], suffix) {
			return fields
		}
	}
	return nil
}

// logfmtFields parses key=value and key="quoted value" pairs.
func logfmtFields(line string) map[string]string {
	fields := map[string]string{}
	for len(line) > 0 {
		line = strings.TrimLeft(line, " ")
		eq := strings.IndexByte(line, '=')
		if eq < 0 {
			break
		}
		key := line[:eq]
		rest := line[eq+1:]
		var value string
		if strings.HasPrefix(rest, `"`) {
			unquoted, err := strconv.QuotedPrefix(rest)
			if err != nil {
				break
			}
			value, _ = strconv.Unquote(unquoted)
			rest = rest[len(unquoted):]
		} else if space := strings.IndexByte(rest, ' '); space >= 0 {
			value, rest = rest[:space], rest[space:]
		} else {
			value, rest = rest, ""
		}
		fields[key] = value
		line = rest
	}
	return fields
}
