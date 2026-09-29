package probe

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"strconv"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

// Method selects how an ingress is measured.
type Method string

const (
	// MethodTCP measures the time to complete a TCP handshake with the
	// ingress endpoint. No proxy protocol handshake is performed.
	MethodTCP Method = "tcp"
	// MethodICMP measures one ICMP echo round trip to the ingress address
	// using unprivileged ICMP facilities of the host OS.
	MethodICMP Method = "icmp"
)

// ParseMethod accepts "", "tcp" and "icmp"; the empty string means tcp.
func ParseMethod(value string) (Method, error) {
	switch Method(value) {
	case "", MethodTCP:
		return MethodTCP, nil
	case MethodICMP:
		return MethodICMP, nil
	default:
		return "", fmt.Errorf("probe method must be tcp or icmp")
	}
}

// Error codes reported in IngressResult.ErrorCode / EntranceResult.ErrorCode.
const (
	CodeCanceled        = "CANCELED"
	CodeTimeout         = "TIMEOUT"
	CodeConnectFailed   = "CONNECT_FAILED"
	CodeDNSFailed       = "DNS_FAILED"
	CodeICMPTimeout     = "ICMP_TIMEOUT"
	CodeICMPUnreachable = "ICMP_UNREACHABLE"
	CodeICMPUnsupported = "ICMP_UNSUPPORTED"
	CodeICMPFailed      = "ICMP_FAILED"
)

// Error carries a stable probe error code.
type Error struct {
	Code string
	Err  error
}

func (e *Error) Error() string {
	if e.Err != nil {
		return e.Code + ": " + e.Err.Error()
	}
	return e.Code
}
func (e *Error) Unwrap() error { return e.Err }

// IngressResult is one replica's measurement, listed in failover order.
type IngressResult struct {
	EndpointKey    string              `json:"endpoint_key"`
	ReplicaOrdinal int                 `json:"replica_ordinal"`
	Role           profile.IngressRole `json:"role"`
	Success        bool                `json:"success"`
	LatencyMS      int64               `json:"latency_ms"`
	ErrorCode      string              `json:"error_code,omitempty"`
}

// EntranceResult is the per-logical-node result. The node-level fields report
// the primary (first) ingress when it succeeded, otherwise the fastest
// successful backup; when every ingress failed they report the primary's
// failure. EndpointKey / IngressRole identify the replica they describe.
type EntranceResult struct {
	NodeID      string              `json:"node_id"`
	Method      Method              `json:"method"`
	Success     bool                `json:"success"`
	LatencyMS   int64               `json:"latency_ms"`
	ErrorCode   string              `json:"error_code,omitempty"`
	EndpointKey string              `json:"endpoint_key"`
	IngressRole profile.IngressRole `json:"ingress_role"`
	Ingresses   []IngressResult     `json:"ingresses"`
	MeasuredAt  time.Time           `json:"measured_at"`
}

type DialContext func(context.Context, string, string) (net.Conn, error)

// PingFunc sends one ICMP echo to addr and returns the round-trip time.
// Failures should be *Error values with an ICMP_* code.
type PingFunc func(ctx context.Context, addr netip.Addr, timeout time.Duration) (time.Duration, error)

// ResolveFunc resolves an ingress domain when the profile carries no IP.
type ResolveFunc func(ctx context.Context, host string) ([]netip.Addr, error)

type Options struct {
	Method      Method
	Timeout     time.Duration
	Concurrency int
	// Test hooks; nil selects the OS implementation.
	Dial    DialContext
	Ping    PingFunc
	Resolve ResolveFunc
}

// Entrances measures every ingress of every node. Concurrency bounds the
// number of ingress probes in flight.
func Entrances(ctx context.Context, p *profile.Profile, opts Options) ([]EntranceResult, error) {
	if err := profile.Validate(p, time.Now()); err != nil {
		return nil, err
	}
	method, err := ParseMethod(string(opts.Method))
	if err != nil {
		return nil, err
	}
	if opts.Timeout <= 0 {
		opts.Timeout = 5 * time.Second
	}
	if opts.Concurrency < 1 {
		opts.Concurrency = 4
	}
	if opts.Dial == nil {
		opts.Dial = (&net.Dialer{}).DialContext
	}
	if opts.Ping == nil {
		opts.Ping = Ping
	}
	if opts.Resolve == nil {
		opts.Resolve = func(ctx context.Context, host string) ([]netip.Addr, error) {
			return net.DefaultResolver.LookupNetIP(ctx, "ip", host)
		}
	}
	results := make([]EntranceResult, len(p.Nodes))
	for i, n := range p.Nodes {
		results[i] = EntranceResult{NodeID: n.ID, Method: method, Ingresses: make([]IngressResult, len(n.Ingresses))}
	}
	sem := make(chan struct{}, opts.Concurrency)
	var wg sync.WaitGroup
	for i, n := range p.Nodes {
		for j, ingress := range n.Ingresses {
			wg.Add(1)
			go func(target *IngressResult, ingress profile.Ingress) {
				defer wg.Done()
				target.EndpointKey, target.ReplicaOrdinal, target.Role = ingress.EndpointKey, ingress.ReplicaOrdinal, ingress.Role
				select {
				case sem <- struct{}{}:
				case <-ctx.Done():
					target.ErrorCode = CodeCanceled
					return
				}
				defer func() { <-sem }()
				if ctx.Err() != nil {
					target.ErrorCode = CodeCanceled
					return
				}
				latency, err := probeIngress(ctx, method, ingress, opts)
				if err != nil {
					target.ErrorCode = errorCode(ctx, method, err)
					return
				}
				target.Success = true
				// Rounded to the nearest millisecond; a success is never 0.
				target.LatencyMS = max(1, (latency + time.Millisecond/2).Milliseconds())
			}(&results[i].Ingresses[j], ingress)
		}
	}
	wg.Wait()
	now := time.Now()
	for i := range results {
		summarize(&results[i])
		results[i].MeasuredAt = now
	}
	return results, nil
}

func summarize(r *EntranceResult) {
	chosen := 0
	if !r.Ingresses[0].Success {
		for j := 1; j < len(r.Ingresses); j++ {
			in := r.Ingresses[j]
			if in.Success && (!r.Ingresses[chosen].Success || in.LatencyMS < r.Ingresses[chosen].LatencyMS) {
				chosen = j
			}
		}
	}
	in := r.Ingresses[chosen]
	r.Success, r.LatencyMS, r.ErrorCode, r.EndpointKey, r.IngressRole = in.Success, in.LatencyMS, in.ErrorCode, in.EndpointKey, in.Role
}

func probeIngress(ctx context.Context, method Method, ingress profile.Ingress, opts Options) (time.Duration, error) {
	attempt, cancel := context.WithTimeout(ctx, opts.Timeout)
	defer cancel()
	addr, err := targetAddress(attempt, ingress.Endpoint, opts.Resolve)
	if err != nil {
		return 0, err
	}
	if method == MethodICMP {
		remaining := opts.Timeout
		if deadline, ok := attempt.Deadline(); ok {
			remaining = time.Until(deadline)
		}
		if remaining <= 0 {
			return 0, context.DeadlineExceeded
		}
		return opts.Ping(attempt, addr, remaining)
	}
	started := time.Now()
	conn, err := opts.Dial(attempt, "tcp", net.JoinHostPort(addr.String(), strconv.Itoa(int(ingress.Endpoint.Port))))
	elapsed := time.Since(started)
	if err != nil {
		return 0, err
	}
	_ = conn.Close()
	return elapsed, nil
}

// targetAddress prefers the literal ingress IP so the probe never depends on
// DNS; otherwise the domain is resolved before timing starts. IPv4 answers
// are preferred because they are the most widely routable.
func targetAddress(ctx context.Context, endpoint profile.Endpoint, resolve ResolveFunc) (netip.Addr, error) {
	if endpoint.IP != "" {
		addr, err := netip.ParseAddr(endpoint.IP)
		if err != nil {
			return netip.Addr{}, &Error{Code: CodeDNSFailed, Err: err}
		}
		return addr.Unmap(), nil
	}
	addrs, err := resolve(ctx, endpoint.Domain)
	if err != nil {
		if ctx.Err() != nil {
			return netip.Addr{}, ctx.Err()
		}
		return netip.Addr{}, &Error{Code: CodeDNSFailed, Err: err}
	}
	var fallback netip.Addr
	for _, addr := range addrs {
		addr = addr.Unmap()
		if addr.Is4() {
			return addr, nil
		}
		if !fallback.IsValid() {
			fallback = addr
		}
	}
	if !fallback.IsValid() {
		return netip.Addr{}, &Error{Code: CodeDNSFailed, Err: errors.New("no addresses")}
	}
	return fallback, nil
}

func errorCode(parent context.Context, method Method, err error) string {
	var coded *Error
	if errors.As(err, &coded) {
		return coded.Code
	}
	if errors.Is(err, context.Canceled) || errors.Is(parent.Err(), context.Canceled) {
		return CodeCanceled
	}
	timeout := errors.Is(err, context.DeadlineExceeded)
	var ne net.Error
	if errors.As(err, &ne) && ne.Timeout() {
		timeout = true
	}
	switch {
	case method == MethodICMP && timeout:
		return CodeICMPTimeout
	case method == MethodICMP:
		return CodeICMPFailed
	case timeout:
		return CodeTimeout
	default:
		return CodeConnectFailed
	}
}

// Address returns the host:port an ingress is probed at (IP when present).
func Address(in profile.Ingress) string {
	host := in.Endpoint.IP
	if host == "" {
		host = in.Endpoint.Domain
	}
	return net.JoinHostPort(host, strconv.Itoa(int(in.Endpoint.Port)))
}
