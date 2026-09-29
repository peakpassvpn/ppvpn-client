package runtime

import (
	"errors"
	"strings"
)

// StageError records which lifecycle stage an internal failure came from.
// The API still folds such failures into CORE_OPERATION_FAILED; the stage
// and the full cause chain only go to the core log.
type StageError struct {
	Stage string
	Err   error
}

func (e *StageError) Error() string { return e.Stage + ": " + e.Err.Error() }
func (e *StageError) Unwrap() error { return e.Err }

func stageError(stage string, err error) error {
	if err == nil {
		return nil
	}
	return &StageError{Stage: stage, Err: err}
}

// Stages returns every stage on err's chain, outermost first, joined with
// " > " (for example "start > engine-start/tun-open"), or "" when none.
func Stages(err error) string {
	var stages []string
	for err != nil {
		if stage, ok := err.(*StageError); ok {
			stages = append(stages, stage.Stage)
		}
		err = errors.Unwrap(err)
	}
	return strings.Join(stages, " > ")
}
