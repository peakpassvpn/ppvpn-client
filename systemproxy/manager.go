// Package systemproxy owns the device-local, unauthenticated application proxy endpoint.
package systemproxy

import (
	"encoding/json"
	"fmt"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"sync"
)

type Endpoint struct {
	Listen string `json:"listen"`
	Port   uint16 `json:"port"`
}

type Endpoints struct {
	HTTP   Endpoint `json:"http"`
	SOCKS5 Endpoint `json:"socks5"`
}

type diskState struct {
	Version  int      `json:"version"`
	Endpoint Endpoint `json:"endpoint"`
}

type Manager struct {
	mu   sync.Mutex
	path string
}

func NewManager(path string) *Manager { return &Manager{path: path} }

func ValidateListen(listen string) error {
	address, err := netip.ParseAddr(listen)
	if err != nil || !address.IsLoopback() {
		return fmt.Errorf("system proxy listen address must be an explicit loopback IP")
	}
	return nil
}

func (m *Manager) Ensure(listen string, reconcile bool) (Endpoint, bool, error) {
	return m.EnsureAvoiding(listen, reconcile, nil)
}

func (m *Manager) EnsureAvoiding(listen string, reconcile bool, reserved map[uint16]bool) (Endpoint, bool, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if err := ValidateListen(listen); err != nil {
		return Endpoint{}, false, err
	}
	state, err := m.load()
	if err != nil {
		return Endpoint{}, false, err
	}
	changed := false
	endpoint := state.Endpoint
	if endpoint.Listen != listen || endpoint.Port == 0 {
		endpoint = Endpoint{Listen: listen}
		changed = true
	}
	if endpoint.Port == 0 || reserved[endpoint.Port] || (reconcile && !portAvailable(endpoint)) {
		port, allocErr := availablePort(listen, reserved)
		if allocErr != nil {
			return Endpoint{}, false, allocErr
		}
		endpoint.Port = port
		changed = true
	}
	if changed {
		if err = m.save(diskState{Version: 1, Endpoint: endpoint}); err != nil {
			return Endpoint{}, false, err
		}
	}
	return endpoint, changed, nil
}

func Both(endpoint Endpoint) Endpoints { return Endpoints{HTTP: endpoint, SOCKS5: endpoint} }

func (m *Manager) load() (diskState, error) {
	state := diskState{Version: 1}
	data, err := os.ReadFile(m.path)
	if os.IsNotExist(err) {
		return state, nil
	}
	if err != nil {
		return state, fmt.Errorf("read system proxy state: %w", err)
	}
	info, err := os.Stat(m.path)
	if err != nil {
		return state, err
	}
	if runtime.GOOS != "windows" && info.Mode().Perm() != 0o600 {
		return state, fmt.Errorf("system proxy state permissions must be 0600")
	}
	if err = json.Unmarshal(data, &state); err != nil || state.Version != 1 {
		return state, fmt.Errorf("unsupported system proxy state")
	}
	if state.Endpoint.Port != 0 {
		if err = ValidateListen(state.Endpoint.Listen); err != nil {
			return state, fmt.Errorf("invalid system proxy state: %w", err)
		}
	}
	return state, nil
}

func (m *Manager) save(state diskState) error {
	dir := filepath.Dir(m.path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	if runtime.GOOS != "windows" {
		if err := os.Chmod(dir, 0o700); err != nil {
			return err
		}
	}
	if runtime.GOOS != "windows" {
		if info, err := os.Stat(dir); err != nil || info.Mode().Perm() != 0o700 {
			if err != nil {
				return err
			}
			return fmt.Errorf("system proxy state directory permissions must be 0700")
		}
	}
	data, err := json.Marshal(state)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".system-proxy-*")
	if err != nil {
		return err
	}
	name := tmp.Name()
	defer os.Remove(name)
	if err = tmp.Chmod(0o600); err == nil {
		_, err = tmp.Write(data)
	}
	if closeErr := tmp.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	return os.Rename(name, m.path)
}

func portAvailable(endpoint Endpoint) bool {
	listener, err := net.Listen("tcp", net.JoinHostPort(endpoint.Listen, strconv.Itoa(int(endpoint.Port))))
	if err != nil {
		return false
	}
	_ = listener.Close()
	return true
}

func availablePort(listen string, reserved map[uint16]bool) (uint16, error) {
	for attempt := 0; attempt < 32; attempt++ {
		listener, err := net.Listen("tcp", net.JoinHostPort(listen, "0"))
		if err != nil {
			return 0, err
		}
		port := uint16(listener.Addr().(*net.TCPAddr).Port)
		_ = listener.Close()
		if !reserved[port] {
			return port, nil
		}
	}
	return 0, fmt.Errorf("could not allocate a system proxy port distinct from local proxies")
}
