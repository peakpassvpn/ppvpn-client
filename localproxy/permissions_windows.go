//go:build windows

package localproxy

import "os"

// Windows access is governed by the ACL inherited from the App's per-user
// private state directory rather than POSIX mode bits.
func securePermissions(_ os.FileInfo) bool { return true }
func secureDirectory(_ string) error       { return nil }
