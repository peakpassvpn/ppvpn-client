//go:build localdns_testsource

package localdns

import (
	"context"
	"encoding/json"
	"errors"
	"net/netip"
	"os"

	"github.com/sagernet/sing/common/control"
)

// TestSourceMarker is in every binary built with this source; release
// checks fail on it (see .github/workflows/release.yml).
const TestSourceMarker = "ppvpn-localdns-testsource-enabled"

// TestSourceEnv names the JSON file a lab build reads the resolvers from:
// {"<interface name>": ["10.0.0.53", "[fe80::1]:53", ...]}. Only builds
// with the localdns_testsource tag have this source; release builds never.
const TestSourceEnv = "PPVPN_LOCALDNS_TEST_FILE"

const testSource = true

func init() {
	discover = discoverTestFile
}

func discoverTestFile(_ context.Context, iface control.Interface) ([]netip.AddrPort, string, error) {
	path := os.Getenv(TestSourceEnv)
	if path == "" {
		return nil, TestSourceMarker, errors.New(TestSourceEnv + " is not set")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, TestSourceMarker, err
	}
	var entries map[string][]string
	if err := json.Unmarshal(data, &entries); err != nil {
		return nil, TestSourceMarker, err
	}
	var servers []netip.AddrPort
	for _, entry := range entries[iface.Name] {
		server, err := netip.ParseAddrPort(entry)
		if err != nil {
			addr, addrErr := netip.ParseAddr(entry)
			if addrErr != nil {
				return nil, TestSourceMarker, err
			}
			server = netip.AddrPortFrom(addr, 53)
		}
		servers = append(servers, server)
	}
	return servers, TestSourceMarker, nil
}
