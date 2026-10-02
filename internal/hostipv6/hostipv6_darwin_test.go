package hostipv6

import (
	"slices"
	"syscall"
	"testing"

	rib "golang.org/x/net/route"
)

func TestDarwinDefaultRouteIndexes(t *testing.T) {
	zero := &rib.Inet6Addr{}
	gateway := &rib.Inet6Addr{IP: [16]byte{0: 0xfe, 1: 0x80, 15: 1}}
	half := &rib.Inet6Addr{IP: [16]byte{0: 0x80}}
	msg := func(index, flags int, dst, mask rib.Addr) rib.Message {
		addrs := make([]rib.Addr, syscall.RTAX_MAX)
		addrs[syscall.RTAX_DST], addrs[syscall.RTAX_GATEWAY], addrs[syscall.RTAX_NETMASK] = dst, gateway, mask
		return &rib.RouteMessage{Index: index, Flags: flags, Addrs: addrs}
	}
	up := syscall.RTF_UP | syscall.RTF_GATEWAY
	got := defaultRouteIndexes([]rib.Message{
		msg(4, up, zero, nil),                        // en0 default, mask omitted
		msg(5, up, zero, zero),                       // explicit ::/0
		msg(9, up, half, half),                       // a TUN's 8000::/1
		msg(6, up|syscall.RTF_REJECT, zero, nil),     // reject
		msg(7, syscall.RTF_GATEWAY, zero, nil),       // down
		msg(8, up|syscall.RTF_BLACKHOLE, zero, zero), // blackhole
	})
	if !slices.Equal(got, []int{4, 5}) {
		t.Fatalf("got %v, want [4 5]", got)
	}
}

func TestRouteOnThisHost(t *testing.T) {
	got, err := Route()
	t.Logf("host IPv6 route: %v (%v)", got, err)
}
