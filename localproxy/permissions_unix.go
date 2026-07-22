//go:build !windows

package localproxy

import "os"

func securePermissions(info os.FileInfo) bool { return info.Mode().Perm()&0o077 == 0 }
func secureDirectory(path string) error       { return os.Chmod(path, 0o700) }
