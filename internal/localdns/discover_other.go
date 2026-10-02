//go:build !windows && !darwin

package localdns

import (
	"context"
	"errors"
	"net/netip"

	"github.com/sagernet/sing/common/control"
)

// discover is replaced in builds with the localdns_testsource tag; elsewhere
// the config renders sing-box's local transport instead (see Supported).
var discover discoverFunc = func(context.Context, control.Interface) ([]netip.AddrPort, string, error) {
	return nil, "unsupported", errors.New("reading the interface's DNS servers is not supported on this platform")
}
