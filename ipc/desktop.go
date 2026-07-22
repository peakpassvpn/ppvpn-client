package ipc

import (
	"context"
	"errors"
	"net"
	"net/http"
	"time"
)

type DesktopServer struct {
	server   *http.Server
	listener net.Listener
}

func Listen(address string, handler http.Handler) (*DesktopServer, error) {
	listener, err := listenLocal(address)
	if err != nil {
		return nil, err
	}
	return &DesktopServer{server: &http.Server{Handler: handler, ReadHeaderTimeout: 5 * time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 16 << 10}, listener: listener}, nil
}
func (s *DesktopServer) Serve() error {
	err := s.server.Serve(s.listener)
	if errors.Is(err, http.ErrServerClosed) {
		return nil
	}
	return err
}
func (s *DesktopServer) Shutdown(ctx context.Context) error { return s.server.Shutdown(ctx) }
func (s *DesktopServer) Address() string                    { return s.listener.Addr().String() }
