// Package privateacl keeps device-local secrets (the local proxy state, its
// directory and the session secret) private to the account the core runs as.
//
// On Unix that is mode 0700 for directories and 0600 for files. On Windows
// it is a protected DACL (no inherited ACEs) that grants full access to:
//   - the current process user and LocalSystem, for a core running as a
//     normal user (the standard core under %LOCALAPPDATA%);
//   - LocalSystem and BUILTIN\Administrators, for a core running as
//     LocalSystem (the privileged service core under ProgramData).
package privateacl

import (
	"errors"
	"fmt"
	"sync/atomic"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
)

var logger atomic.Pointer[corelog.Logger]

// SetLogger sets the diagnostic log that records a successful repair of the
// ppvpn-core 0.4.0 ACL. Only paths and ACLs are logged, never file contents.
// A nil logger (the default) logs nothing.
func SetLogger(l *corelog.Logger) { logger.Store(l) }

// ErrNotPrivate reports an existing file whose permissions are broader than
// private. It is never repaired automatically: the secret may have leaked.
var ErrNotPrivate = errors.New("permissions are not private")

// AccessError reports a directory or file this account cannot use and
// cannot repair. Nothing is deleted; Remedy says what an operator can do.
type AccessError struct {
	Path   string
	Err    error
	Remedy string
}

func (e *AccessError) Error() string {
	return fmt.Sprintf("cannot use %s: %v; %s", e.Path, e.Err, e.Remedy)
}
func (e *AccessError) Unwrap() error { return e.Err }
