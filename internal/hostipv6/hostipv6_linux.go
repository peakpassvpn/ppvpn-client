package hostipv6

import (
	"errors"
	"io/fs"
	"os"
	"strings"
)

// disableIPv6Files are the sysctls that decide whether a new interface (the
// TUN) gets IPv6: "all" overrides every interface and "default" is inherited
// by interfaces created later. Without /proc/sys/net/ipv6 at all the kernel
// runs with ipv6.disable=1.
var disableIPv6Files = []string{
	"/proc/sys/net/ipv6/conf/all/disable_ipv6",
	"/proc/sys/net/ipv6/conf/default/disable_ipv6",
}

func available() bool { return availableFrom(os.ReadFile) }

func availableFrom(readFile func(string) ([]byte, error)) bool {
	for _, name := range disableIPv6Files {
		value, err := readFile(name)
		if errors.Is(err, fs.ErrNotExist) {
			return false
		}
		if err != nil {
			// Unreadable but present: keep IPv6 so a leak stays impossible;
			// a genuinely disabled stack then fails the start visibly.
			continue
		}
		if strings.TrimSpace(string(value)) != "0" {
			return false
		}
	}
	return true
}
