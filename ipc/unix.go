//go:build !windows

package ipc

import (
	"fmt"
	"net"
	"os"
	"path/filepath"
)

func listenLocal(path string) (net.Listener, error) {
	if path == "" {
		return nil, fmt.Errorf("unix socket path is required")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	if info, err := os.Lstat(path); err == nil {
		if info.Mode()&os.ModeSocket == 0 {
			return nil, fmt.Errorf("refusing to replace non-socket path")
		}
		if err = os.Remove(path); err != nil {
			return nil, err
		}
	} else if !os.IsNotExist(err) {
		return nil, err
	}
	listener, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	if err = os.Chmod(path, 0o600); err != nil {
		listener.Close()
		return nil, err
	}
	return &cleanupListener{Listener: listener, path: path}, nil
}

type cleanupListener struct {
	net.Listener
	path string
}

func (l *cleanupListener) Close() error { err := l.Listener.Close(); _ = os.Remove(l.path); return err }
