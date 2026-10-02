package hostipv6

import (
	"errors"
	"io/fs"
	"testing"
)

func TestLinuxAvailability(t *testing.T) {
	errDenied := errors.New("permission denied")
	cases := []struct {
		name       string
		all, deflt string
		allErr     error
		want       bool
	}{
		{name: "enabled", all: "0\n", deflt: "0\n", want: true},
		{name: "all disabled", all: "1\n", deflt: "0\n", want: false},
		{name: "default disabled", all: "0\n", deflt: "1\n", want: false},
		{name: "ipv6.disable=1", allErr: fs.ErrNotExist, want: false},
		{name: "unreadable keeps ipv6", allErr: errDenied, deflt: "0\n", want: true},
	}
	for _, tc := range cases {
		got := availableFrom(func(name string) ([]byte, error) {
			switch name {
			case disableIPv6Files[0]:
				return []byte(tc.all), tc.allErr
			case disableIPv6Files[1]:
				if tc.allErr == fs.ErrNotExist {
					return nil, fs.ErrNotExist
				}
				return []byte(tc.deflt), nil
			}
			t.Fatalf("unexpected read %q", name)
			return nil, nil
		})
		if got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
}

func TestLinuxRoute(t *testing.T) {
	const (
		eth0Global = "20010db8000000000000000000000050 02 40 00 80     eth0\n"
		eth0Link   = "fe800000000000000000000000000001 02 40 20 80     eth0\n"
		tunULA     = "fde2ec409312c7fd0000000000000001 05 7e 00 80     tun0\n"
		lo         = "00000000000000000000000000000001 01 80 10 80       lo\n"
		// ::/0 via fe80::1 on eth0, up+gateway.
		defaultEth0 = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003     eth0\n"
		// ::/0 in the TUN's own table.
		defaultTun = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000400 00000001 00000000 00000001     tun0\n"
		// The kernel's reject ::/0 on lo (no route).
		rejectLo = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n"
		// A default route that is a reject route on eth0.
		rejectEth0 = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00000201     eth0\n"
	)
	cases := []struct {
		name          string
		addrs, routes string
		want          bool
	}{
		{name: "global address and default route", addrs: lo + eth0Link + eth0Global, routes: rejectLo + defaultEth0, want: true},
		{name: "no global address (lab, VM 102)", addrs: lo + eth0Link, routes: rejectLo + defaultEth0, want: false},
		{name: "global address, no default route", addrs: lo + eth0Global, routes: rejectLo, want: false},
		{name: "only the TUN has a default route", addrs: lo + eth0Global + tunULA, routes: rejectLo + defaultTun, want: false},
		{name: "reject default route", addrs: eth0Global, routes: rejectEth0, want: false},
	}
	for _, tc := range cases {
		got, err := routeFrom(func(name string) ([]byte, error) {
			switch name {
			case ifInet6File:
				return []byte(tc.addrs), nil
			case ipv6RouteFile:
				return []byte(tc.routes), nil
			}
			t.Fatalf("unexpected read %q", name)
			return nil, nil
		})
		if err != nil || got != tc.want {
			t.Errorf("%s: got %v, %v; want %v", tc.name, got, err, tc.want)
		}
	}
	got, err := routeFrom(func(string) ([]byte, error) { return nil, fs.ErrNotExist })
	if !got || err == nil {
		t.Errorf("unreadable: got %v, %v; want true with an error", got, err)
	}
}
