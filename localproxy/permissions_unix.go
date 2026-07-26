//go:build !windows

package localproxy

import "os"

func securePermissions(_ string, info os.FileInfo) bool { return info.Mode().Perm()&0o077 == 0 }
func secureDirectory(path string) error                 { return os.Chmod(path, 0o700) }
func secureFile(path string) error                      { return os.Chmod(path, 0o600) }
