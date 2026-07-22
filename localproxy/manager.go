package localproxy

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"sort"
	"sync"
)

type Endpoint struct {
	NodeID   string `json:"node_id"`
	Listen   string `json:"listen"`
	Port     uint16 `json:"port"`
	Username string `json:"username"`
	Password string `json:"password"`
}

type diskState struct {
	Version   int                 `json:"version"`
	Endpoints map[string]Endpoint `json:"endpoints"`
}

// Manager persists only device-local proxy credentials and ports. The state
// file must live in an app-private directory and is always written mode 0600.
type Manager struct {
	mu   sync.Mutex
	path string
}

func NewManager(path string) *Manager { return &Manager{path: path} }

func (m *Manager) Ensure(nodeIDs []string) ([]Endpoint, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	state, err := m.load()
	if err != nil {
		return nil, err
	}
	changed := false
	used := map[uint16]bool{}
	for _, endpoint := range state.Endpoints {
		if endpoint.Port == 0 || used[endpoint.Port] {
			return nil, fmt.Errorf("local proxy state contains duplicate or invalid ports")
		}
		used[endpoint.Port] = true
	}
	for _, id := range nodeIDs {
		if _, ok := state.Endpoints[id]; ok {
			continue
		}
		port, err := availablePort(used)
		if err != nil {
			return nil, err
		}
		username, err := randomSecret(18)
		if err != nil {
			return nil, err
		}
		password, err := randomSecret(32)
		if err != nil {
			return nil, err
		}
		state.Endpoints[id] = Endpoint{NodeID: id, Listen: "127.0.0.1", Port: port, Username: username, Password: password}
		used[port] = true
		changed = true
	}
	if changed {
		if err = m.save(state); err != nil {
			return nil, err
		}
	}
	out := make([]Endpoint, 0, len(nodeIDs))
	for _, id := range nodeIDs {
		out = append(out, state.Endpoints[id])
	}
	sort.Slice(out, func(i, j int) bool { return out[i].NodeID < out[j].NodeID })
	return out, nil
}

// ReconcileForStartup preserves every available persisted port and replaces
// only ports already occupied before the core starts listening.
func (m *Manager) ReconcileForStartup(nodeIDs []string) ([]Endpoint, error) {
	_, err := m.Ensure(nodeIDs)
	if err != nil {
		return nil, err
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	state, err := m.load()
	if err != nil {
		return nil, err
	}
	used := map[uint16]bool{}
	for _, endpoint := range state.Endpoints {
		used[endpoint.Port] = true
	}
	changed := false
	for _, id := range nodeIDs {
		endpoint := state.Endpoints[id]
		listener, listenErr := net.Listen("tcp", net.JoinHostPort(endpoint.Listen, fmt.Sprintf("%d", endpoint.Port)))
		if listenErr == nil {
			listener.Close()
			continue
		}
		delete(used, endpoint.Port)
		port, allocErr := availablePort(used)
		if allocErr != nil {
			return nil, allocErr
		}
		endpoint.Port = port
		state.Endpoints[id] = endpoint
		used[port] = true
		changed = true
	}
	if changed {
		if err = m.save(state); err != nil {
			return nil, err
		}
	}
	out := make([]Endpoint, 0, len(nodeIDs))
	for _, id := range nodeIDs {
		out = append(out, state.Endpoints[id])
	}
	sort.Slice(out, func(i, j int) bool { return out[i].NodeID < out[j].NodeID })
	return out, nil
}

func (m *Manager) load() (diskState, error) {
	state := diskState{Version: 1, Endpoints: map[string]Endpoint{}}
	data, err := os.ReadFile(m.path)
	if os.IsNotExist(err) {
		return state, nil
	}
	if err != nil {
		return state, fmt.Errorf("read local proxy state: %w", err)
	}
	info, err := os.Stat(m.path)
	if err != nil {
		return state, err
	}
	if !securePermissions(info) {
		return state, fmt.Errorf("local proxy state permissions must be 0600")
	}
	if err = json.Unmarshal(data, &state); err != nil {
		return state, fmt.Errorf("decode local proxy state: %w", err)
	}
	if state.Version != 1 || state.Endpoints == nil {
		return state, fmt.Errorf("unsupported local proxy state")
	}
	return state, nil
}

func (m *Manager) save(state diskState) error {
	dir := filepath.Dir(m.path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	if err := secureDirectory(dir); err != nil {
		return err
	}
	data, err := json.Marshal(state)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".local-proxy-*")
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

func availablePort(used map[uint16]bool) (uint16, error) {
	for attempt := 0; attempt < 32; attempt++ {
		listener, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			return 0, err
		}
		port := uint16(listener.Addr().(*net.TCPAddr).Port)
		listener.Close()
		if !used[port] {
			return port, nil
		}
	}
	return 0, fmt.Errorf("could not allocate unique loopback port")
}
func randomSecret(bytes int) (string, error) {
	value := make([]byte, bytes)
	if _, err := rand.Read(value); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(value), nil
}
