package localproxy

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"github.com/peakpassvpn/ppvpn-core/internal/privateacl"
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
// "<prefix>-<alias>" (see Aliases) selects the node; the password is one
// per-device secret shared by all nodes.
const (
	Listen = "127.0.0.1"
	// PreferredPort is tried first on a fresh device and whenever the
	// persisted port is unavailable at startup.
	PreferredPort uint16 = 7890
	// StateVersion 2 replaced per-node ports/credentials (version 1) with one
	// shared port, username prefix and password.
	StateVersion = 2
	prefixLength = 5
	// SystemProxyPreferredPort is the first choice for the optional
	// unauthenticated system proxy listener.
	SystemProxyPreferredPort uint16 = 7891
)

const prefixAlphabet = "abcdefghijklmnopqrstuvwxyz0123456789"

type diskState struct {
	Version  int    `json:"version"`
	Prefix   string `json:"prefix"`
	Password string `json:"password"`
	Port     uint16 `json:"port,omitempty"`
	// SystemProxyPort is the last port of the opt-in system proxy. Whether
	// it is enabled is deliberately not persisted: it starts disabled.
	SystemProxyPort uint16 `json:"system_proxy_port,omitempty"`
}

// legacyState is the version 1 layout, kept only to validate it before an
// in-place upgrade.
type legacyState struct {
	Version   int                        `json:"version"`
	Endpoints map[string]json.RawMessage `json:"endpoints"`
}

// Manager persists only device-local proxy settings: the username prefix,
// the shared password and the last bound ports. The state file must live in an
// app-private directory and is always written mode 0600.
type Manager struct {
	mu            sync.Mutex
	path          string
	preferredPort uint16
	systemPort    uint16
}

func NewManager(path string) *Manager {
	return &Manager{path: path, preferredPort: PreferredPort, systemPort: SystemProxyPreferredPort}
}

// WithSystemProxyPreferredPort overrides SystemProxyPreferredPort; 0 means
// any free loopback port.
func (m *Manager) WithSystemProxyPreferredPort(port uint16) *Manager {
	m.systemPort = port
	return m
}

// SystemProxyPort returns the port for the system proxy listener: the
// persisted port when it is free, otherwise SystemProxyPreferredPort, otherwise
// any free loopback port, never avoid (the shared local proxy port). With
// probe=false a persisted port is returned unchecked, for a listener that is
// already running on it. The choice is persisted.
func (m *Manager) SystemProxyPort(probe bool, avoid uint16) (uint16, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	state, changed, err := m.load()
	if err != nil {
		return 0, err
	}
	port := state.SystemProxyPort
	if port == 0 || port == avoid || probe {
		if port == avoid {
			port = 0
		}
		preferred := m.systemPort
		if preferred == avoid {
			preferred = 0
		}
		if port, err = choosePort(port, preferred); err != nil {
			return 0, err
		}
		if port == avoid {
			// Only reachable when avoid is not bound yet; take another port.
			if port, err = choosePort(0, 0); err != nil {
				return 0, err
			}
		}
	}
	if port != state.SystemProxyPort {
		state.SystemProxyPort = port
		changed = true
	}
	if changed {
		if err = m.save(state); err != nil {
			return 0, err
		}
	}
	return port, nil
}

// WithPreferredPort overrides PreferredPort; 0 means no preferred port, so a
// fresh or displaced port is any free loopback port.
func (m *Manager) WithPreferredPort(port uint16) *Manager {
	m.preferredPort = port
	return m
}

// FormatUsername returns the internal route key "<prefix>-<nodeID>" that the
// rendered config uses as the proxy user and auth_user of nodeID. Clients
// never see or type it; they use the login (see Login and Aliases).
func FormatUsername(prefix, nodeID string) string { return prefix + "-" + nodeID }

// ParseUsername splits a route key or a login into the device prefix and the
// node id or alias. The prefix never contains '-', so the rest is everything
// after the first '-' and may itself contain '-'.
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
// password on first use. Usernames are client logins; RouteEndpoints converts
// them for the config builder. It keeps the persisted port without probing
// it, which is what a running core needs: its own listener holds that port.
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
		if err == nil && port != 0 && port == state.SystemProxyPort {
			// Never share a port with the system proxy listener.
			port, err = choosePort(0, 0)
		}
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
	aliases := Aliases(nodeIDs)
	out := make([]Endpoint, 0, len(nodeIDs))
	for _, id := range nodeIDs {
		out = append(out, Endpoint{NodeID: id, Listen: Listen, Port: state.Port, Username: Login(state.Prefix, aliases[id]), Password: state.Password})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].NodeID < out[j].NodeID })
	return out, nil
}

// load reads the state, upgrading a version 1 file in place: its per-node
// ports and credentials are dropped, and changed reports that the caller must
// persist the regenerated settings.
func (m *Manager) load() (state diskState, changed bool, err error) {
	state = diskState{Version: StateVersion}
	// Secure (and, if it carries the 0.4.0 ACL, repair) the directory and
	// check the file before reading: an unreadable file must fail with the
	// path and a remedy, not a bare "Access is denied".
	if err = m.prepareDirectory(); err != nil {
		return state, false, err
	}
	exists, err := privateacl.CheckFile(m.path)
	if err != nil {
		return state, false, fmt.Errorf("local proxy state: %w", err)
	}
	if !exists {
		return state, false, nil
	}
	data, err := os.ReadFile(m.path)
	if err != nil {
		return state, false, fmt.Errorf("read local proxy state: %w", err)
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

// prepareDirectory creates the state directory and applies the private
// ACL, which also repairs a directory this account owns but was locked out
// of by an older ACL.
func (m *Manager) prepareDirectory() error {
	dir := filepath.Dir(m.path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return fmt.Errorf("create local proxy state directory: %w", err)
	}
	if err := privateacl.SecureDirectory(dir); err != nil {
		return fmt.Errorf("secure local proxy state directory: %w", err)
	}
	return nil
}

func (m *Manager) save(state diskState) error {
	dir := filepath.Dir(m.path)
	if err := m.prepareDirectory(); err != nil {
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
		err = privateacl.SecureFile(name)
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
