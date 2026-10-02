package hostipv6

import (
	"net/netip"
	"testing"
)

func TestWindowsAvailability(t *testing.T) {
	cases := []struct {
		name     string
		value    uint32
		set      bool
		adapters bool
		want     bool
	}{
		{name: "default", adapters: true, want: true},
		{name: "prefer ipv4 only", value: 0x20, set: true, adapters: true, want: true},
		{name: "tunnel interfaces only", value: 0x01, set: true, adapters: true, want: true},
		{name: "non-tunnel disabled", value: 0x10, set: true, adapters: true, want: false},
		{name: "all disabled", value: 0xFF, set: true, adapters: true, want: false},
		{name: "stack missing", adapters: false, want: false},
	}
	for _, tc := range cases {
		got := availableFrom(func() (uint32, bool) { return tc.value, tc.set }, func() bool { return tc.adapters })
		if got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
}

func TestWindowsRoute(t *testing.T) {
	global := netip.MustParseAddr("2001:db8::50")
	linkLocal := netip.MustParseAddr("fe80::1")
	ula := netip.MustParseAddr("fde2:ec40:9312:c7fd::1")
	cases := []struct {
		name     string
		adapters []adapterIPv6
		want     bool
	}{
		{name: "ethernet with global address and gateway", adapters: []adapterIPv6{{up: true, gateway: true, addrs: []netip.Addr{linkLocal, global}}}, want: true},
		{name: "VM 102: link-local only, Wintun ULA", adapters: []adapterIPv6{{up: true, gateway: false, addrs: []netip.Addr{linkLocal}}, {up: true, gateway: true, addrs: []netip.Addr{ula}}}, want: false},
		{name: "global address without gateway", adapters: []adapterIPv6{{up: true, addrs: []netip.Addr{global}}}, want: false},
		{name: "adapter down", adapters: []adapterIPv6{{up: false, gateway: true, addrs: []netip.Addr{global}}}, want: false},
		{name: "no adapters", want: false},
	}
	for _, tc := range cases {
		if got := routeFrom(tc.adapters); got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
}
