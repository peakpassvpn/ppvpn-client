package localdns

import (
	"bytes"
	"context"
	"net/netip"
	"os/exec"
	"strings"
	"time"

	"github.com/sagernet/sing/common/control"
)

// scutilTimeout bounds one scutil run.
const scutilTimeout = 2 * time.Second

var discover discoverFunc = discoverScutil

// discoverScutil reads the primary service's DNS when it is iface's, and
// otherwise (another VPN is primary, configd is behind, no servers) the
// scoped resolver of iface. No cgo: scutil prints what SystemConfiguration
// holds, manual servers included, which DHCP-only sources (ipconfig
// getpacket, sing-box's dhcp transport) miss on static networks.
func discoverScutil(ctx context.Context, iface control.Interface) ([]netip.AddrPort, string, error) {
	output, err := runScutil(ctx, scutilScript)
	if err != nil {
		return nil, "scutil-global", err
	}
	if servers, ok := globalServers(output, iface.Name, iface.Index); ok && len(servers) > 0 {
		return servers, "scutil-global", nil
	}
	output, err = runScutil(ctx, "", "--dns")
	if err != nil {
		return nil, "scutil-scoped", err
	}
	return scopedServers(output, iface.Index), "scutil-scoped", nil
}

func runScutil(ctx context.Context, stdin string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, scutilTimeout)
	defer cancel()
	command := exec.CommandContext(ctx, "/usr/sbin/scutil", args...)
	command.Stdin = strings.NewReader(stdin)
	var stdout bytes.Buffer
	command.Stdout = &stdout
	if err := command.Run(); err != nil {
		return "", err
	}
	return stdout.String(), nil
}
