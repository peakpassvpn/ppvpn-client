//go:build !windows

package privateacl

import (
	"fmt"
	"os"
)

// SecureDirectory restricts an existing directory to the current account.
func SecureDirectory(path string) error { return os.Chmod(path, 0o700) }

// SecureFile restricts an existing file to the current account.
func SecureFile(path string) error { return os.Chmod(path, 0o600) }

// CheckFile reports whether path exists and, if so, that it is private.
func CheckFile(path string) (bool, error) {
	info, err := os.Stat(path)
	if os.IsNotExist(err) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	if info.Mode().Perm()&0o077 != 0 {
		return true, fmt.Errorf("%s: %w", path, ErrNotPrivate)
	}
	return true, nil
}
