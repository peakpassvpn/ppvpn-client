package config

import (
	"bytes"
	"context"
	"reflect"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/domaindest"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/profile"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	singjson "github.com/sagernet/sing/common/json"
)

func buildHostIPv6(t *testing.T, platform string, opts BuildOptions) *BuildResult {
	t.Helper()
	got, err := BuildWithOptions(tunRoutingProfile("direct"), profile.PlatformCapabilities{Platform: platform, TUN: profile.TUNCapabilities{Enabled: true}}, opts, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func outboundByTag(r *BuildResult, tag string) (option.Outbound, bool) {
	for _, o := range r.Options.Outbounds {
		if o.Tag == tag {
			return o, true
		}
	}
	return option.Outbound{}, false
}

func tunInbound(t *testing.T, r *BuildResult) []byte {
	t.Helper()
	for _, inbound := range r.Options.Inbounds {
		if inbound.Tag == TUNInboundTag {
			raw, err := singjson.MarshalContext(failover.Context(context.Background()), inbound)
			if err != nil {
				t.Fatal(err)
			}
			return raw
		}
	}
	t.Fatal("no tun inbound")
	return nil
}

// A host with IPv6 enabled but no IPv6 path of its own keeps the same TUN
// (IPv6 address and routes, so nothing bypasses it) and only wraps direct.
func TestNoHostIPv6RouteHandsDirectIPv6ItsDomain(t *testing.T) {
	for _, platform := range []string{"windows", "linux", "macos"} {
		t.Run(platform, func(t *testing.T) {
			with := buildHostIPv6(t, platform, BuildOptions{})
			without := buildHostIPv6(t, platform, BuildOptions{NoHostIPv6Route: true})

			if !bytes.Equal(tunInbound(t, with), tunInbound(t, without)) {
				t.Fatalf("TUN inbound differs:\n%s\n%s", tunInbound(t, with), tunInbound(t, without))
			}
			if with.DirectIPv6HandOff || !without.DirectIPv6HandOff {
				t.Fatalf("hand-off flags: with route %v, without %v", with.DirectIPv6HandOff, without.DirectIPv6HandOff)
			}

			// With an IPv6 path: direct is the plain outbound, as before.
			direct, _ := outboundByTag(with, "direct")
			if direct.Type != C.TypeDirect || direct.Options.(*option.DirectOutboundOptions).DomainResolver != nil {
				t.Fatalf("direct with an IPv6 route: %#v", direct)
			}
			if _, ok := outboundByTag(with, DirectHostTag); ok {
				t.Fatal("direct-host rendered on a host with an IPv6 route")
			}

			// Without: direct wraps direct-host, which resolves IPv4 only.
			wrapper, _ := outboundByTag(without, "direct")
			options, ok := wrapper.Options.(*domaindest.Options)
			if wrapper.Type != domaindest.Type || !ok || options.Outbound != DirectHostTag || !options.IPv6Only || !reflect.DeepEqual(options.Inbounds, []string{TUNInboundTag}) {
				t.Fatalf("direct without an IPv6 route: %#v", wrapper)
			}
			host, _ := outboundByTag(without, DirectHostTag)
			resolver := host.Options.(*option.DirectOutboundOptions).DomainResolver
			if host.Type != C.TypeDirect || resolver == nil || resolver.Server != DNSLocalTag || resolver.Strategy != option.DomainStrategy(C.DomainStrategyIPv4Only) {
				t.Fatalf("direct-host: %#v", host)
			}

			// Everything else is identical: rules, DNS and route.final still
			// name "direct".
			if !reflect.DeepEqual(with.Options.Route, without.Options.Route) || !reflect.DeepEqual(with.Options.DNS, without.Options.DNS) {
				t.Fatal("route or DNS options differ")
			}

			// The rendered options decode through the core's registry.
			ctx := failover.Context(context.Background())
			raw, err := singjson.MarshalContext(ctx, without.Options)
			if err != nil {
				t.Fatal(err)
			}
			var decoded option.Options
			if err = singjson.UnmarshalContext(ctx, raw, &decoded); err != nil {
				t.Fatalf("rendered options do not decode: %v", err)
			}
		})
	}
}

// The hand-off needs the TUN's IPv6: a host with IPv6 disabled, and mobile
// (IPv4-only tunnel), render direct as before.
func TestNoHostIPv6RouteLeavesIPv4OnlyTUNAlone(t *testing.T) {
	cases := []struct {
		platform string
		opts     BuildOptions
	}{
		{"windows", BuildOptions{NoHostIPv6Route: true, DisableTUNIPv6: true}},
		{"ios", BuildOptions{NoHostIPv6Route: true}},
		{"android", BuildOptions{NoHostIPv6Route: true}},
	}
	for _, tc := range cases {
		got := buildHostIPv6(t, tc.platform, tc.opts)
		if direct, _ := outboundByTag(got, "direct"); got.DirectIPv6HandOff || direct.Type != C.TypeDirect {
			t.Errorf("%s %+v: direct %#v, hand-off %v", tc.platform, tc.opts, direct, got.DirectIPv6HandOff)
		}
	}
	// Without a TUN nothing changes either.
	got, err := BuildWithOptions(tunRoutingProfile("direct"), profile.PlatformCapabilities{Platform: "windows"}, BuildOptions{NoHostIPv6Route: true}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if got.DirectIPv6HandOff {
		t.Error("hand-off without a TUN")
	}
}
