//go:build !linux || android

package tunrules

// Only desktop Linux routes the TUN by policy rules sing-tun installs;
// elsewhere (Android's TUN is the VpnService's) there is nothing to guard.

type Logger interface {
	Info(msg string, fields ...any)
	Warn(msg string, fields ...any)
	Error(msg string, fields ...any)
}

type Guard struct{}

func Start(Scope, Logger, func(State)) (*Guard, error) { return nil, nil }

func (*Guard) Check(string) {}

func (*Guard) Close() {}
