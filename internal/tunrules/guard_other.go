//go:build !linux

package tunrules

// Only Linux routes the TUN by policy rules; elsewhere there is nothing to
// guard.

type Logger interface {
	Info(msg string, fields ...any)
	Warn(msg string, fields ...any)
	Error(msg string, fields ...any)
}

type Guard struct{}

func Start(Scope, Logger, func(State)) (*Guard, error) { return nil, nil }

func (*Guard) Check(string) {}

func (*Guard) Close() {}
