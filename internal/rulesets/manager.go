// Package rulesets downloads, verifies and caches the profile's sing-box
// binary rule sets. The core manages the files itself instead of using
// sing-box remote rule sets, so that downloads are pinned to the profile's
// host and digest, always go direct, survive restarts, and a missing set
// degrades the configuration instead of failing it.
package rulesets

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/version"
	"github.com/sagernet/sing-box/common/srs"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
)

type State string

const (
	// StateReady: the local copy matches the profile's sha256 and is in use.
	StateReady State = "ready"
	// StateStale: the profile's version could not be fetched; an earlier
	// verified copy (another sha256) is in use.
	StateStale State = "stale"
	// StateUnavailable: no copy exists; rules naming the set skip it.
	StateUnavailable State = "unavailable"
)

// Error codes reported in Status.Error.
const (
	ErrHostNotPinned      = "RULE_SET_HOST_NOT_PINNED"
	ErrDownloadFailed     = "RULE_SET_DOWNLOAD_FAILED"
	ErrHTTPStatus         = "RULE_SET_HTTP_STATUS"
	ErrTooLarge           = "RULE_SET_TOO_LARGE"
	ErrSHA256Mismatch     = "RULE_SET_SHA256_MISMATCH"
	ErrInvalid            = "RULE_SET_INVALID"
	ErrStorage            = "RULE_SET_STORAGE_FAILED"
	ErrStorageUnavailable = "RULE_SET_STORAGE_UNAVAILABLE"
)

// MaxSize bounds one downloaded rule set.
const MaxSize = 32 << 20

// Status is one rule set's state as reported by get-status.
type Status struct {
	ID        string     `json:"id"`
	State     State      `json:"state"`
	UpdatedAt *time.Time `json:"updated_at,omitempty"`
	Error     string     `json:"error,omitempty"`
	// Failures counts consecutive failed downloads (omitted when zero).
	Failures int `json:"failures,omitempty"`
	// NextRetryAt is when a set that is not ready is retried; omitted when
	// ready, or when nothing can change before the next apply-profile
	// (RULE_SET_HOST_NOT_PINNED, RULE_SET_STORAGE_UNAVAILABLE).
	NextRetryAt *time.Time `json:"next_retry_at,omitempty"`
}

// DialFunc opens a direct connection that bypasses the tunnel.
type DialFunc func(ctx context.Context, network, address string) (net.Conn, error)

type Options struct {
	// Dir holds <id>.srs. Empty disables downloads and caching.
	Dir string
	// Dial opens the direct connections downloads use. Nil uses net.Dialer.
	Dial DialFunc
	// TLSConfig overrides the TLS client configuration (tests).
	TLSConfig *tls.Config
	// OnState is called (without locks held) when a set changes state.
	OnState func(Status)
	// OnRebuild is called (on its own goroutine) when a refresh changed
	// which rule sets are available or how they must be rendered, so the
	// configuration must be rebuilt. Content changes of a set that is
	// already rendered need no rebuild: sing-box watches local rule set
	// files and reloads them in place.
	OnRebuild func()
	// RetryMin and RetryMax bound the retry backoff of sets that are not
	// ready: RetryMin, doubling per consecutive failure up to RetryMax
	// (defaults 5s and 15m, never above the set's update interval).
	RetryMin, RetryMax time.Duration
	// FetchTimeout bounds one background download (default 60s).
	FetchTimeout time.Duration
	Now          func() time.Time
}

type Manager struct {
	opts Options

	mu         sync.Mutex
	generation uint64
	current    *Snapshot
	cancel     context.CancelFunc
}

// Snapshot is the state of one profile's rule sets. Prepare returns one;
// Activate makes it current.
type Snapshot struct {
	pinned  map[string]bool
	entries []*entry
}

type entry struct {
	set       profile.RuleSet
	state     State
	updatedAt time.Time
	err       string
	local     *localCopy
	failures  int
	due       time.Time
}

type localCopy struct {
	path      string
	sha256    string
	mirrorDNS bool
}

func New(opts Options) *Manager {
	if opts.RetryMin <= 0 {
		opts.RetryMin = 5 * time.Second
	}
	if opts.RetryMax <= 0 {
		opts.RetryMax = 15 * time.Minute
	}
	if opts.FetchTimeout <= 0 {
		opts.FetchTimeout = time.Minute
	}
	if opts.Now == nil {
		opts.Now = time.Now
	}
	return &Manager{opts: opts}
}

// Prepare resolves every rule set to a verified local copy. A cached copy
// matching the profile's sha256 is used at once; otherwise, when download is
// set and the url's host is pinned, the set is fetched within ctx. A failed
// fetch falls back to an older cached copy (stale) or leaves the set
// unavailable. Prepare never fails: rule sets can only degrade routing.
func (m *Manager) Prepare(ctx context.Context, sets []profile.RuleSet, allowedHosts []string, download bool) *Snapshot {
	snapshot := &Snapshot{pinned: map[string]bool{}}
	for _, host := range allowedHosts {
		if normalized, err := profile.NormalizeRuleSetHost(host); err == nil {
			snapshot.pinned[normalized] = true
		}
	}
	// Copy the previous entries under the lock: the refresh loop updates
	// them in place.
	m.mu.Lock()
	previous := map[string]entry{}
	if m.current != nil {
		for _, e := range m.current.entries {
			previous[e.set.ID] = *e
		}
	}
	m.mu.Unlock()
	now := m.opts.Now()
	var wait sync.WaitGroup
	for _, set := range sets {
		e := &entry{set: set, state: StateUnavailable}
		snapshot.entries = append(snapshot.entries, e)
		if before, ok := previous[set.ID]; ok && before.set == set {
			e.err, e.failures = before.err, before.failures
		}
		if m.opts.Dir == "" {
			e.err = ErrStorageUnavailable
			continue
		}
		e.local = m.readLocal(set.ID)
		if e.local != nil && e.local.sha256 == set.SHA256Hex() {
			e.state, e.err, e.failures = StateReady, "", 0
			e.updatedAt = modTime(e.local.path, now)
			continue
		}
		if e.local != nil {
			e.state = StateStale
		}
		if !snapshot.hostPinned(set) {
			e.err = ErrHostNotPinned
			continue
		}
		if !download {
			continue
		}
		wait.Add(1)
		go func() {
			defer wait.Done()
			m.fetchInto(ctx, e)
		}()
	}
	wait.Wait()
	for _, e := range snapshot.entries {
		e.due = m.nextDue(e, now)
	}
	return snapshot
}

// Files returns the verified local copy of every set that has one.
func (s *Snapshot) Files() map[string]config.RuleSetFile {
	files := map[string]config.RuleSetFile{}
	for _, e := range s.entries {
		if e.local != nil && e.state != StateUnavailable {
			files[e.set.ID] = config.RuleSetFile{Path: e.local.path, MirrorDNS: e.local.mirrorDNS}
		}
	}
	return files
}

func (s *Snapshot) hostPinned(set profile.RuleSet) bool {
	host, err := profile.RuleSetHost(set.URL)
	return err == nil && s.pinned[host]
}

// Activate makes snapshot current, reports state changes, removes cached
// files of sets the profile no longer names, and (re)starts the refresh
// loop.
func (m *Manager) Activate(snapshot *Snapshot) {
	m.mu.Lock()
	if m.cancel != nil {
		m.cancel()
		m.cancel = nil
	}
	previous := map[string]State{}
	if m.current != nil {
		for _, e := range m.current.entries {
			previous[e.set.ID] = e.state
		}
	}
	m.generation++
	m.current = snapshot
	var changed []Status
	keep := map[string]bool{}
	for _, e := range snapshot.entries {
		keep[e.set.ID+".srs"] = true
		if before, ok := previous[e.set.ID]; !ok || before != e.state {
			changed = append(changed, e.status())
		}
	}
	if len(snapshot.entries) > 0 {
		ctx, cancel := context.WithCancel(context.Background())
		m.cancel = cancel
		go m.loop(ctx, m.generation)
	}
	m.mu.Unlock()
	m.prune(keep)
	m.notify(changed)
}

// Close stops the refresh loop.
func (m *Manager) Close() {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.cancel != nil {
		m.cancel()
		m.cancel = nil
	}
	m.generation++
}

// Statuses reports the current sets in profile order.
func (m *Manager) Statuses() []Status {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.current == nil {
		return nil
	}
	out := make([]Status, len(m.current.entries))
	for i, e := range m.current.entries {
		out[i] = e.status()
	}
	return out
}

func (e *entry) status() Status {
	status := Status{ID: e.set.ID, State: e.state}
	if !e.updatedAt.IsZero() {
		at := e.updatedAt.UTC()
		status.UpdatedAt = &at
	}
	if e.state != StateReady {
		status.Error = e.err
		status.Failures = e.failures
		if !e.due.IsZero() && e.err != ErrHostNotPinned && e.err != ErrStorageUnavailable {
			at := e.due.UTC()
			status.NextRetryAt = &at
		}
	}
	return status
}

// Counts summarizes a snapshot's sets by state (apply timing).
func (s *Snapshot) Counts() (ready, stale, unavailable int) {
	for _, e := range s.entries {
		switch e.state {
		case StateReady:
			ready++
		case StateStale:
			stale++
		default:
			unavailable++
		}
	}
	return
}

func (m *Manager) notify(changed []Status) {
	if m.opts.OnState == nil {
		return
	}
	for _, status := range changed {
		m.opts.OnState(status)
	}
}

// refreshConcurrency bounds the parallel downloads of one refresh round.
const refreshConcurrency = 4

// loop refreshes every set when it is due: ready sets on their update
// interval, the others with a bounded backoff. Due sets are refreshed
// concurrently. When a set that was not ready recovers, the connectivity it
// was waiting for is probably back, so every other set that is not ready is
// retried at once instead of on its own backoff. The configuration is rebuilt
// once, after the refreshes that are due right now have all finished, not
// once per set (a rebuild replaces the engine and drops open connections).
func (m *Manager) loop(ctx context.Context, generation uint64) {
	rebuildPending := false
	for {
		m.mu.Lock()
		if m.generation != generation {
			m.mu.Unlock()
			return
		}
		var next time.Time
		var due []*entry
		now := m.opts.Now()
		for _, e := range m.current.entries {
			if !e.due.After(now) {
				due = append(due, e)
			} else if next.IsZero() || e.due.Before(next) {
				next = e.due
			}
		}
		snapshot := m.current
		m.mu.Unlock()
		if len(due) == 0 {
			if rebuildPending && m.opts.OnRebuild != nil {
				go m.opts.OnRebuild()
			}
			rebuildPending = false
			if next.IsZero() {
				return
			}
			timer := time.NewTimer(next.Sub(now))
			select {
			case <-ctx.Done():
				timer.Stop()
				return
			case <-timer.C:
			}
			continue
		}
		var wg sync.WaitGroup
		var resultMu sync.Mutex
		recovered := false
		slots := make(chan struct{}, refreshConcurrency)
		for _, e := range due {
			wg.Add(1)
			slots <- struct{}{}
			go func(e *entry) {
				defer wg.Done()
				defer func() { <-slots }()
				rebuild, back := m.refresh(ctx, generation, snapshot, e)
				resultMu.Lock()
				rebuildPending = rebuildPending || rebuild
				recovered = recovered || back
				resultMu.Unlock()
			}(e)
		}
		wg.Wait()
		if ctx.Err() != nil {
			return
		}
		if recovered {
			m.mu.Lock()
			if m.generation == generation {
				now := m.opts.Now()
				for _, e := range m.current.entries {
					if e.state != StateReady && e.err != ErrHostNotPinned && e.err != ErrStorageUnavailable && e.due.After(now) {
						e.due = now
					}
				}
			}
			m.mu.Unlock()
		}
	}
}

// refresh fetches one set. rebuild reports that the configuration must be
// rebuilt; recovered that a set which was not ready now is.
func (m *Manager) refresh(ctx context.Context, generation uint64, snapshot *Snapshot, e *entry) (rebuild, recovered bool) {
	m.mu.Lock()
	candidate := *e
	m.mu.Unlock()
	if m.opts.Dir != "" && snapshot.hostPinned(candidate.set) {
		fetchCtx, cancel := context.WithTimeout(ctx, m.opts.FetchTimeout)
		m.fetchInto(fetchCtx, &candidate)
		cancel()
	}
	if ctx.Err() != nil {
		return false, false
	}
	m.mu.Lock()
	if m.generation != generation {
		m.mu.Unlock()
		return false, false
	}
	before := *e
	*e = candidate
	e.due = m.nextDue(e, m.opts.Now())
	status := e.status()
	m.mu.Unlock()
	if before.state != e.state {
		m.notify([]Status{status})
	}
	rebuild = (before.state == StateUnavailable) != (e.state == StateUnavailable) ||
		(before.local != nil && e.local != nil && before.local.mirrorDNS != e.local.mirrorDNS)
	return rebuild, before.state != StateReady && e.state == StateReady
}

func (m *Manager) nextDue(e *entry, now time.Time) time.Time {
	interval := e.set.UpdateInterval()
	if e.state == StateReady {
		return now.Add(interval)
	}
	if e.err == ErrHostNotPinned || e.err == ErrStorageUnavailable {
		// Nothing can change until the next apply-profile.
		return now.Add(interval)
	}
	backoff := m.opts.RetryMin
	for i := 1; i < e.failures && backoff < m.opts.RetryMax; i++ {
		backoff *= 2
	}
	return now.Add(min(backoff, m.opts.RetryMax, interval))
}

// fetchInto downloads e's set and, when it verifies, installs it as the
// local copy. e is not shared while this runs.
func (m *Manager) fetchInto(ctx context.Context, e *entry) {
	want := e.set.SHA256Hex()
	var cached string
	if e.local != nil {
		cached = e.local.sha256
	}
	body, notModified, code := m.fetch(ctx, e.set, cached)
	now := m.opts.Now()
	if code == "" && notModified && cached != want {
		// The server still serves the cached (older) version.
		code = ErrSHA256Mismatch
	}
	if code == "" && !notModified {
		code = m.install(e, body, want)
	}
	if code != "" {
		e.err = code
		e.failures++
		return
	}
	e.state, e.err, e.failures, e.updatedAt = StateReady, "", 0, now
}

func (m *Manager) fetch(ctx context.Context, set profile.RuleSet, cachedSHA string) ([]byte, bool, string) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, set.URL, nil)
	if err != nil {
		return nil, false, ErrDownloadFailed
	}
	request.Header.Set("Accept", "application/octet-stream")
	request.Header.Set("User-Agent", "ppvpn-core/"+version.Get().CoreVersion)
	if cachedSHA != "" {
		request.Header.Set("If-None-Match", `"`+cachedSHA+`"`)
	}
	response, err := m.client().Do(request)
	if err != nil {
		return nil, false, ErrDownloadFailed
	}
	defer response.Body.Close()
	switch {
	case response.StatusCode == http.StatusNotModified && cachedSHA != "":
		return nil, true, ""
	case response.StatusCode != http.StatusOK:
		return nil, false, ErrHTTPStatus
	}
	body, err := io.ReadAll(io.LimitReader(response.Body, MaxSize+1))
	if err != nil {
		return nil, false, ErrDownloadFailed
	}
	if len(body) > MaxSize {
		return nil, false, ErrTooLarge
	}
	return body, false, ""
}

func (m *Manager) client() *http.Client {
	dial := m.opts.Dial
	if dial == nil {
		dial = (&net.Dialer{Timeout: 10 * time.Second}).DialContext
	}
	var tlsConfig *tls.Config
	if m.opts.TLSConfig != nil {
		tlsConfig = m.opts.TLSConfig.Clone()
	}
	return &http.Client{
		Transport: &http.Transport{
			// Never an environment proxy: downloads go direct.
			Proxy:               nil,
			DialContext:         dial,
			TLSClientConfig:     tlsConfig,
			TLSHandshakeTimeout: 10 * time.Second,
			DisableKeepAlives:   true,
			ForceAttemptHTTP2:   true,
		},
		// A redirect would leave the pinned host.
		CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
	}
}

// install verifies body against want and atomically replaces the local copy.
func (m *Manager) install(e *entry, body []byte, want string) string {
	sum := sha256.Sum256(body)
	if hex.EncodeToString(sum[:]) != want {
		return ErrSHA256Mismatch
	}
	mirrorDNS, err := inspect(body)
	if err != nil {
		return ErrInvalid
	}
	path, ok := m.path(e.set.ID)
	if !ok {
		return ErrStorage
	}
	if err = writeAtomic(path, body); err != nil {
		return ErrStorage
	}
	e.local = &localCopy{path: path, sha256: want, mirrorDNS: mirrorDNS}
	return ""
}

// path returns the file of rule set id inside Dir. The profile already
// restricts ids to a safe alphabet; this keeps the file inside Dir even if a
// caller skipped validation.
func (m *Manager) path(id string) (string, bool) {
	dir := filepath.Clean(m.opts.Dir)
	path := filepath.Clean(filepath.Join(dir, id+".srs"))
	if !strings.HasPrefix(path, dir+string(filepath.Separator)) || filepath.Dir(path) != dir {
		return "", false
	}
	return path, true
}

// readLocal loads and verifies the cached copy of id, or returns nil.
func (m *Manager) readLocal(id string) *localCopy {
	path, ok := m.path(id)
	if !ok {
		return nil
	}
	data, err := os.ReadFile(path)
	if err != nil || len(data) > MaxSize {
		return nil
	}
	mirrorDNS, err := inspect(data)
	if err != nil {
		return nil
	}
	sum := sha256.Sum256(data)
	return &localCopy{path: path, sha256: hex.EncodeToString(sum[:]), mirrorDNS: mirrorDNS}
}

func (m *Manager) prune(keep map[string]bool) {
	if m.opts.Dir == "" {
		return
	}
	entries, err := os.ReadDir(m.opts.Dir)
	if err != nil {
		return
	}
	for _, entry := range entries {
		name := entry.Name()
		if !entry.IsDir() && (strings.HasSuffix(name, ".srs") && !keep[name] || strings.HasPrefix(name, ".tmp-")) {
			_ = os.Remove(filepath.Join(m.opts.Dir, name))
		}
	}
}

// inspect parses a binary rule set and reports whether it may be mirrored
// into DNS rules: it matches domains and no destination IP CIDRs.
func inspect(data []byte) (bool, error) {
	compat, err := srs.Read(bytes.NewReader(data), false)
	if err != nil {
		return false, err
	}
	if compat.Version > C.RuleSetVersionCurrent {
		return false, fmt.Errorf("unsupported rule set version %d", compat.Version)
	}
	plain, err := compat.Upgrade()
	if err != nil {
		return false, err
	}
	var domains, cidrs bool
	var walk func([]option.HeadlessRule)
	walk = func(rules []option.HeadlessRule) {
		for _, rule := range rules {
			switch rule.Type {
			case C.RuleTypeLogical:
				walk(rule.LogicalOptions.Rules)
			default:
				r := rule.DefaultOptions
				if len(r.Domain)+len(r.DomainSuffix)+len(r.DomainKeyword)+len(r.DomainRegex)+len(r.AdGuardDomain) > 0 || r.DomainMatcher != nil || r.AdGuardDomainMatcher != nil {
					domains = true
				}
				if len(r.IPCIDR) > 0 || r.IPSet != nil {
					cidrs = true
				}
			}
		}
	}
	walk(plain.Rules)
	return domains && !cidrs, nil
}

func modTime(path string, fallback time.Time) time.Time {
	if info, err := os.Stat(path); err == nil {
		return info.ModTime()
	}
	return fallback
}

// writeAtomic writes data to a private temporary file next to path and
// renames it over path, so readers (sing-box's file watcher included) only
// ever see a complete file.
func writeAtomic(path string, data []byte) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".tmp-*")
	if err != nil {
		return err
	}
	name := tmp.Name()
	defer os.Remove(name)
	if _, err = tmp.Write(data); err == nil {
		err = tmp.Sync()
	}
	if closeErr := tmp.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Chmod(name, 0o600)
	}
	if err != nil {
		return err
	}
	return rename(name, path)
}

// rename retries a failed replace. On Windows a file that is still open
// without FILE_SHARE_DELETE cannot be replaced; sing-box 1.13 leaves the
// handle of a loaded binary rule set to the garbage collector, so collect and
// retry briefly before giving up.
func rename(from, to string) error {
	var err error
	for attempt := 0; attempt < 10; attempt++ {
		if err = os.Rename(from, to); err == nil {
			return nil
		}
		if !errors.Is(err, os.ErrPermission) && runtime.GOOS != "windows" {
			return err
		}
		runtime.GC()
		time.Sleep(50 * time.Millisecond)
	}
	return err
}
