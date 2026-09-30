package dnslog

import (
	"context"
	"errors"
	"strings"
	"testing"

	mDNS "github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/log"
)

type fakeTransport struct {
	adapter.DNSTransport
	response *mDNS.Msg
	err      error
}

func (f fakeTransport) Tag() string  { return "dns-local" }
func (f fakeTransport) Type() string { return C.DNSTypeLocal }
func (f fakeTransport) Exchange(context.Context, *mDNS.Msg) (*mDNS.Msg, error) {
	return f.response, f.err
}

func construct(t *testing.T, ctx context.Context, inner adapter.DNSTransport) adapter.DNSTransport {
	t.Helper()
	got, err := wrap(func(context.Context, log.ContextLogger, string, struct{}) (adapter.DNSTransport, error) {
		return inner, nil
	})(ctx, nil, "dns-local", struct{}{})
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func TestExchangeIsLoggedOnlyAtDebugLevel(t *testing.T) {
	query := new(mDNS.Msg)
	query.SetQuestion("example.com.", mDNS.TypeAAAA)
	response := new(mDNS.Msg)
	response.SetRcode(query, mDNS.RcodeNameError)

	var b strings.Builder
	l := corelog.New(&b)
	transport := construct(t, WithLogger(context.Background(), l), fakeTransport{response: response})
	if transport.Type() != C.DNSTypeLocal {
		t.Fatalf("type not forwarded: %q", transport.Type())
	}
	_, _ = transport.Exchange(context.Background(), query)
	if b.Len() != 0 {
		t.Fatalf("logged at info level: %q", b.String())
	}
	_ = l.SetLevel(corelog.LevelDebug)
	_, _ = transport.Exchange(context.Background(), query)
	if !strings.Contains(b.String(), "msg=dns name=example.com. type=AAAA server=dns-local rcode=NXDOMAIN answers=0 ms=") {
		t.Fatalf("line: %q", b.String())
	}

	b.Reset()
	failing := construct(t, WithLogger(context.Background(), l), fakeTransport{err: errors.New("i/o timeout")})
	_, _ = failing.Exchange(context.Background(), query)
	if !strings.Contains(b.String(), `server=dns-local error="i/o timeout" ms=`) {
		t.Fatalf("error line: %q", b.String())
	}
}

// Without a logger in the context the transport is returned unwrapped.
func TestNoLoggerLeavesTransportUnwrapped(t *testing.T) {
	inner := fakeTransport{}
	if got := construct(t, context.Background(), inner); got != adapter.DNSTransport(inner) {
		t.Fatalf("wrapped without a logger: %T", got)
	}
}
