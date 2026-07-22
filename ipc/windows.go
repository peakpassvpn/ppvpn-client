//go:build windows

package ipc

import (
	"fmt"
	"github.com/Microsoft/go-winio"
	"net"
)

func listenLocal(path string) (net.Listener, error) {
	if path == "" {
		return nil, fmt.Errorf("named pipe path is required")
	}
	return winio.ListenPipe(path, &winio.PipeConfig{SecurityDescriptor: "D:P(A;;GA;;;OW)", MessageMode: false, InputBufferSize: 65536, OutputBufferSize: 65536})
}
