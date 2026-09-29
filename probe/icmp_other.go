//go:build !darwin && !linux && !windows

package probe

import (
	"context"
	"errors"
	"net/netip"
	"time"
)

// Ping is unavailable on this platform.
func Ping(context.Context, netip.Addr, time.Duration) (time.Duration, error) {
	return 0, &Error{Code: CodeICMPUnsupported, Err: errors.New("unprivileged ICMP is not supported on this platform")}
}
