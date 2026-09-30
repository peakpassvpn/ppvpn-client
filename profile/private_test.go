package profile

import (
	"net/netip"
	"testing"
)

func TestIsPrivateIP(t *testing.T) {
	for _, ip := range []string{"10.1.2.3", "172.16.0.1", "192.168.1.1", "100.64.0.1", "0.1.2.3", "127.0.0.1", "169.254.169.254", "224.0.0.251", "240.0.0.1", "255.255.255.255", "fc00::1", "fe80::1%en0", "ff02::fb", "::1", "::ffff:10.0.0.1"} {
		if !IsPrivateIP(netip.MustParseAddr(ip)) {
			t.Errorf("%s not private", ip)
		}
	}
	for _, ip := range []string{"8.8.8.8", "100.128.0.1", "198.18.0.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"} {
		if IsPrivateIP(netip.MustParseAddr(ip)) {
			t.Errorf("%s private", ip)
		}
	}
}
