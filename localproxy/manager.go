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
	"strconv"
	"strings"
	"sync"
)

// Endpoint is how a client reaches one node. All endpoints of a device share
// Listen, Port and Password; Username selects the node.
type Endpoint struct {
	NodeID   string `json:"node_id"`
	Listen   string `json:"listen"`
	Port     uint16 `json:"port"`
	Username string `json:"username"`
	Password string `json:"password"`
}

type Metadata struct {
	NodeID       string   `json:"node_id"`
	Listen       string   `json:"listen"`
	Port         uint16   `json:"port"`
	Protocols    []string `json:"protocols"`
	AuthRequired bool     `json:"auth_required"`
}

type Credential struct {
	NodeID   string `json:"node_id"`
	Listen   string `json:"listen"`
	Port     uint16 `json:"port"`
	Username string `json:"username"`
	Password string `json:"password"`
}

// Every node is served on one shared loopback port. The proxy username
// "<prefix>-<nodeID>" selects the node; the password is one per-device secret
// shared by all nodes.
const (
	Listen = "127.0.0.1"
	// PreferredPort is tried first on a fresh device and whenever the
	// persisted port is unavailable at startup.
	PreferredPort uint16 = 7890
	// StateVersion 2 replaced per-node ports/credentials (version 1) with one
	// shared port, username prefix and password.
	StateVersion = 2
	prefixLength = 5
)

const prefixAlphabet = "abcdefghijklmnopqrstuvwxyz0123456789"

type diskState struct {
	Version  int    `json:"version"`
	Prefix   string `json:"prefix"`
	Password string `json:"password"`
	Port     uint16 `json:"port,omitempty"`
}

// legacyState is the version 1 layout, kept only to validate it before an
// in-place upgrade.
type legacyState struct {
	Version   int                        `json:"version"`
	Endpoints map[string]json.RawMessage `json:"endpoints"`
}

// Manager persists only device-local proxy settings: the username prefix,
// the shared password and the last bound port. The state file must live in an
// app-private directory and is always written mode 0600.
type Manager struct {
	mu            sync.Mutex
	path          string
	preferredPort uint16
}

func NewManager(path string) *Manager { return &Manager{path: path, preferredPort: PreferredPort} }

// WithPreferredPort overrides PreferredPort; 0 means no preferred port, so a
// fresh or displaced port is any free loopback port.
func (m *Manager) WithPreferredPort(port uint16) *Manager {
	m.preferredPort = port
	return m
}

// FormatUsername returns the proxy username that selects nodeID.
func FormatUsername(prefix, nodeID string) string { return prefix + "-" + nodeID }

// ParseUsername splits a proxy username into the device prefix and node id.
// The prefix never contains '-', so the node id is everything after the first
// '-' and may itself contain '-'.
func ParseUsername(username string) (prefix, nodeID string, ok bool) {
	prefix, nodeID, ok = strings.Cut(username, "-")
	if !ok || !validPrefix(prefix) || nodeID == "" {
		return "", "", false
	}
	return prefix, nodeID, true
}

func validPrefix(prefix string) bool {
	if len(prefix) != prefixLength {
		return false
	}
	for i := 0; i < len(prefix); i++ {
		if !strings.ContainsRune(prefixAlphabet, rune(prefix[i])) {
			return false
		}
	}
	return true
}

// Ensure returns the endpoints for nodeIDs, generating the device prefix and
// password on first use. It keeps the persisted port without probing it, which
// is what a running core needs: its own listener holds that port.
func (m *Manager) Ensure(nodeIDs []string) ([]Endpoint, error) {
	return m.prepare(nodeIDs, false)
}

// ReconcileForStartup is Ensure for a core that is not listening yet: it
// keeps the persisted port when it is free, otherwise tries PreferredPort and
// then any free loopback port, and persists the result so it is tried first
// next time.
func (m *Manager) ReconcileForStartup(nodeIDs []string) ([]Endpoint, error) {
	return m.prepare(nodeIDs, true)
}

func (m *Manager) prepare(nodeIDs []string, probe bool) ([]Endpoint, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	state, changed, err := m.load()
	if err != nil {
		return nil, err
	}
	if state.Prefix == "" {
		if state.Prefix, err = randomPrefix(); err != nil {
			return nil, err
		}
		changed = true
	}
	if state.Password == "" {
		if state.Password, err = randomSecret(32); err != nil {
			return nil, err
		}
		changed = true
	}
	if state.Port == 0 || probe {
		port, err := choosePort(state.Port, m.preferredPort)
		if err != nil {
			return nil, err
		}
		if port != state.Port {
			state.Port = port
			changed = true
		}
	}
	if changed {
		if err = m.save(state); err != nil {
			return nil, err
		}
	}
	out := make([]Endpoint, 0, len(nodeIDs))
	for _, id := range nodeIDs {
		out = append(out, Endpoint{NodeID: id, Listen: Listen, Port: state.Port, Username: FormatUsername(state.Prefix, id), Password: state.Password})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].NodeID < out[j].NodeID })
	return out, nil
}

// load reads the state, upgrading a version 1 file in place: its per-node
// ports and credentials are dropped, and changed reports that the caller must
// persist the regenerated settings.
func (m *Manager) load() (state diskState, changed bool, err error) {
	state = diskState{Version: StateVersion}
	data, err := os.ReadFile(m.path)
	if os.IsNotExist(err) {
		return state, false, nil
	}
	if err != nil {
		return state, false, fmt.Errorf("read local proxy state: %w", err)
	}
	info, err := os.Stat(m.path)
	if err != nil {
		return state, false, err
	}
	if !securePermissions(m.path, info) {
		return state, false, fmt.Errorf("local proxy state permissions are not private")
	}
	var header struct {
		Version int `json:"version"`
	}
	if err = json.Unmarshal(data, &header); err != nil {
		return state, false, fmt.Errorf("decode local proxy state: %w", err)
	}
	switch header.Version {
	case 1:
		var legacy legacyState
		if err = json.Unmarshal(data, &legacy); err != nil || legacy.Endpoints == nil {
			return state, false, fmt.Errorf("unsupported local proxy state")
		}
		return diskState{Version: StateVersion}, true, nil
	case StateVersion:
		if err = json.Unmarshal(data, &state); err != nil {
			return state, false, fmt.Errorf("decode local proxy state: %w", err)
		}
		if (state.Prefix != "" && !validPrefix(state.Prefix)) || (state.Prefix == "") != (state.Password == "") {
			return state, false, fmt.Errorf("unsupported local proxy state")
		}
		return state, false, nil
	default:
		return state, false, fmt.Errorf("unsupported local proxy state")
	}
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
		err = secureFile(name)
	}
	if err == nil {
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

// choosePort returns the first free loopback port among persisted, preferred
// and a kernel-assigned ephemeral port. Zero candidates are skipped.
func choosePort(persisted, preferred uint16) (uint16, error) {
	for _, candidate := range []uint16{persisted, preferred} {
		if candidate != 0 && portFree(candidate) {
			return candidate, nil
		}
	}
	listener, err := net.Listen("tcp", net.JoinHostPort(Listen, "0"))
	if err != nil {
		return 0, fmt.Errorf("could not allocate loopback port: %w", err)
	}
	defer listener.Close()
	return uint16(listener.Addr().(*net.TCPAddr).Port), nil
}

func portFree(port uint16) bool {
	listener, err := net.Listen("tcp", net.JoinHostPort(Listen, strconv.Itoa(int(port))))
	if err != nil {
		return false
	}
	listener.Close()
	return true
}

func randomPrefix() (string, error) {
	// Rejection sampling keeps every character equally likely.
	out := make([]byte, 0, prefixLength)
	limit := byte(256 - 256%len(prefixAlphabet))
	buffer := make([]byte, 16)
	for len(out) < prefixLength {
		if _, err := rand.Read(buffer); err != nil {
			return "", err
		}
		for _, b := range buffer {
			if b < limit && len(out) < prefixLength {
				out = append(out, prefixAlphabet[int(b)%len(prefixAlphabet)])
			}
		}
	}
	return string(out), nil
}

func randomSecret(bytes int) (string, error) {
	value := make([]byte, bytes)
	if _, err := rand.Read(value); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(value), nil
}
