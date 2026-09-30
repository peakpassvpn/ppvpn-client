package runtime

import (
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
)

// phaseTimer records how long each phase of a lifecycle operation took, for
// one info line per operation ("start timing", "apply timing"). A nil timer
// records nothing.
type phaseTimer struct {
	started, last time.Time
	fields        []any
}

func newPhaseTimer() *phaseTimer {
	now := time.Now()
	return &phaseTimer{started: now, last: now}
}

// mark ends the phase that began at the previous mark (or at creation).
func (p *phaseTimer) mark(phase string) {
	if p == nil {
		return
	}
	now := time.Now()
	p.fields = append(p.fields, phase+"_ms", now.Sub(p.last).Milliseconds())
	p.last = now
}

// log writes the phases with the total and the outcome.
func (p *phaseTimer) log(log *corelog.Logger, msg string, err error, extra ...any) {
	if p == nil {
		return
	}
	outcome := "ok"
	if err != nil {
		outcome = "failed"
	}
	fields := append([]any{"outcome", outcome}, extra...)
	fields = append(fields, p.fields...)
	fields = append(fields, "total_ms", time.Since(p.started).Milliseconds())
	log.Info(msg, fields...)
}
