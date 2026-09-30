// Package dnslog wraps the core's DNS transports so that, at debug level, every
// exchange sent to an upstream server writes one line to the first-party
// diagnostic log: the query name and type, the server tag, the rcode (or the
// error) and how long it took.
//
// The transports are registered under their usual type names, so options and
// golden files are unchanged; only the constructor wraps what sing-box builds.
// Hijacked queries answered from the DNS cache never reach a transport and are
// not logged.
package dnslog

import (
	"context"
	"time"

	mDNS "github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/dns"
	"github.com/sagernet/sing-box/dns/transport"
	"github.com/sagernet/sing-box/dns/transport/local"
	"github.com/sagernet/sing-box/log"
	"github.com/sagernet/sing-box/option"
)

type loggerKey struct{}

// WithLogger makes the DNS transports created under ctx log to l.
func WithLogger(ctx context.Context, l *corelog.Logger) context.Context {
	return context.WithValue(ctx, loggerKey{}, l)
}

func loggerFrom(ctx context.Context) *corelog.Logger {
	l, _ := ctx.Value(loggerKey{}).(*corelog.Logger)
	return l
}

// Register replaces the constructors of the transport types the core uses
// (and the plain remote ones its tests substitute) with logging ones.
func Register(registry *dns.TransportRegistry) {
	dns.RegisterTransport[option.LocalDNSServerOptions](registry, C.DNSTypeLocal, wrap(local.NewTransport))
	dns.RegisterTransport[option.RemoteTLSDNSServerOptions](registry, C.DNSTypeTLS, wrap(transport.NewTLS))
	dns.RegisterTransport[option.RemoteHTTPSDNSServerOptions](registry, C.DNSTypeHTTPS, wrap(transport.NewHTTPS))
	dns.RegisterTransport[option.RemoteDNSServerOptions](registry, C.DNSTypeTCP, wrap(transport.NewTCP))
	dns.RegisterTransport[option.RemoteDNSServerOptions](registry, C.DNSTypeUDP, wrap(transport.NewUDP))
}

func wrap[T any](constructor dns.TransportConstructorFunc[T]) dns.TransportConstructorFunc[T] {
	return func(ctx context.Context, logger log.ContextLogger, tag string, options T) (adapter.DNSTransport, error) {
		inner, err := constructor(ctx, logger, tag, options)
		if err != nil {
			return nil, err
		}
		l := loggerFrom(ctx)
		if l == nil {
			return inner, nil
		}
		return &loggedTransport{DNSTransport: inner, log: l}, nil
	}
}

// loggedTransport forwards everything to the wrapped transport. sing-box only
// inspects transports through Type() (fake-ip), which is forwarded.
type loggedTransport struct {
	adapter.DNSTransport
	log *corelog.Logger
}

func (t *loggedTransport) Exchange(ctx context.Context, message *mDNS.Msg) (*mDNS.Msg, error) {
	if !t.log.DebugEnabled() {
		return t.DNSTransport.Exchange(ctx, message)
	}
	started := time.Now()
	response, err := t.DNSTransport.Exchange(ctx, message)
	name, qtype := "", ""
	if len(message.Question) > 0 {
		name, qtype = message.Question[0].Name, mDNS.TypeToString[message.Question[0].Qtype]
	}
	fields := []any{"name", name, "type", qtype, "server", t.Tag()}
	switch {
	case err != nil:
		fields = append(fields, "error", err)
	case response != nil:
		fields = append(fields, "rcode", mDNS.RcodeToString[response.Rcode], "answers", len(response.Answer))
	}
	t.log.Debug("dns", append(fields, "ms", time.Since(started).Milliseconds())...)
	return response, err
}
