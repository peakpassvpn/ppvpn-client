package runtime

import (
	"context"

	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/include"
	"github.com/sagernet/sing-box/option"
)

type engine interface {
	Start() error
	Close() error
}
type telemetryEngine interface {
	telemetrySnapshot() (Traffic, []Connection)
}
type singEngine struct {
	*box.Box
	tracker *telemetry
}

func (e *singEngine) telemetrySnapshot() (Traffic, []Connection) { return e.tracker.snapshot() }

type engineFactory func(context.Context, option.Options) (engine, error)

func newSingBox(ctx context.Context, options option.Options) (engine, error) {
	instance, err := box.New(box.Options{Context: include.Context(ctx), Options: options})
	if err != nil {
		return nil, err
	}
	tracker := newTelemetry()
	instance.Router().AppendTracker(tracker)
	return &singEngine{Box: instance, tracker: tracker}, nil
}
