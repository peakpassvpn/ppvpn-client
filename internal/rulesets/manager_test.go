package rulesets

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/sagernet/sing-box/common/srs"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

// BuildSRS encodes a binary rule set (version 3) matching the given domain
// suffixes and CIDRs.
func buildSRS(t *testing.T, suffixes, cidrs []string) []byte {
	t.Helper()
	rule := option.DefaultHeadlessRule{DomainSuffix: badoption.Listable[string](suffixes), IPCIDR: badoption.Listable[string](cidrs)}
	var buffer bytes.Buffer
	if err := srs.Write(&buffer, option.PlainRuleSet{Rules: []option.HeadlessRule{{Type: C.RuleTypeDefault, DefaultOptions: rule}}}, C.RuleSetVersion3); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

func digest(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

// ruleSetServer serves one .srs with ETag = sha256 and honors If-None-Match.
type ruleSetServer struct {
	*httptest.Server
	mu          sync.Mutex
	body        []byte
	fail        bool
	requests    atomic.Int32
	notModified atomic.Int32
}

func newRuleSetServer(t *testing.T, body []byte) *ruleSetServer {
	s := &ruleSetServer{body: body}
	s.Server = httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		s.requests.Add(1)
		s.mu.Lock()
		body, fail := s.body, s.fail
		s.mu.Unlock()
		if fail {
			http.Error(w, "unavailable", http.StatusServiceUnavailable)
			return
		}
		etag := `"` + digest(body) + `"`
		w.Header().Set("ETag", etag)
		if r.Header.Get("If-None-Match") == etag {
			s.notModified.Add(1)
			w.WriteHeader(http.StatusNotModified)
			return
		}
		w.Header().Set("Content-Type", "application/octet-stream")
		_, _ = w.Write(body)
	}))
	t.Cleanup(s.Close)
	return s
}

func (s *ruleSetServer) set(body []byte, fail bool) {
	s.mu.Lock()
	s.body, s.fail = body, fail
	s.mu.Unlock()
}

func (s *ruleSetServer) host() string { return s.Listener.Addr().String() }

func (s *ruleSetServer) ruleSet(id string, body []byte) profile.RuleSet {
	return profile.RuleSet{ID: id, URL: s.URL + "/api/v1/proxy-profile/rule-sets/" + id + ".srs", SHA256: digest(body), UpdateIntervalSeconds: 3600}
}

type clock struct {
	mu  sync.Mutex
	now time.Time
}

func (c *clock) Now() time.Time { c.mu.Lock(); defer c.mu.Unlock(); return c.now }
func (c *clock) Add(d time.Duration) {
	c.mu.Lock()
	c.now = c.now.Add(d)
	c.mu.Unlock()
}

func newManager(t *testing.T, server *ruleSetServer, dir string, opts Options) *Manager {
	t.Helper()
	opts.Dir = dir
	opts.TLSConfig = server.Client().Transport.(*http.Transport).TLSClientConfig
	manager := New(opts)
	t.Cleanup(manager.Close)
	return manager
}

func statusOf(t *testing.T, statuses []Status, id string) Status {
	t.Helper()
	for _, status := range statuses {
		if status.ID == id {
			return status
		}
	}
	t.Fatalf("no status for %s in %+v", id, statuses)
	return Status{}
}

func TestPrepareDownloadsVerifiesAndReusesCache(t *testing.T) {
	body := buildSRS(t, []string{"cn.example"}, nil)
	server := newRuleSetServer(t, body)
	dir := t.TempDir()
	set := server.ruleSet("cn-site", body)

	manager := newManager(t, server, dir, Options{})
	snapshot := manager.Prepare(context.Background(), []profile.RuleSet{set}, []string{server.host()}, true)
	files := snapshot.Files()
	if file, ok := files["cn-site"]; !ok || file.Path != filepath.Join(dir, "cn-site.srs") || !file.MirrorDNS {
		t.Fatalf("files: %+v", files)
	}
	if server.requests.Load() != 1 {
		t.Fatalf("requests: %d", server.requests.Load())
	}
	if data, err := os.ReadFile(filepath.Join(dir, "cn-site.srs")); err != nil || !bytes.Equal(data, body) {
		t.Fatalf("cached file: %v", err)
	}
	manager.Activate(snapshot)
	if status := statusOf(t, manager.Statuses(), "cn-site"); status.State != StateReady || status.UpdatedAt == nil || status.Error != "" {
		t.Fatalf("status: %+v", status)
	}

	// A restarted core uses the matching cached copy without the network.
	server.set(nil, true)
	restarted := newManager(t, server, dir, Options{})
	snapshot = restarted.Prepare(context.Background(), []profile.RuleSet{set}, []string{server.host()}, true)
	if _, ok := snapshot.Files()["cn-site"]; !ok || server.requests.Load() != 1 {
		t.Fatalf("cache not reused: files %+v, requests %d", snapshot.Files(), server.requests.Load())
	}
}

func TestPrepareRejectsDigestMismatchAndForeignHosts(t *testing.T) {
	body := buildSRS(t, []string{"cn.example"}, nil)
	server := newRuleSetServer(t, buildSRS(t, []string{"other.example"}, nil))
	dir := t.TempDir()
	manager := newManager(t, server, dir, Options{})

	snapshot := manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("cn-site", body)}, []string{server.host()}, true)
	manager.Activate(snapshot)
	if status := statusOf(t, manager.Statuses(), "cn-site"); status.State != StateUnavailable || status.Error != ErrSHA256Mismatch {
		t.Fatalf("status: %+v", status)
	}
	if _, err := os.Stat(filepath.Join(dir, "cn-site.srs")); !os.IsNotExist(err) {
		t.Fatalf("mismatched download was written: %v", err)
	}

	requests := server.requests.Load()
	snapshot = manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("cn-site", body)}, nil, true)
	manager.Activate(snapshot)
	if status := statusOf(t, manager.Statuses(), "cn-site"); status.State != StateUnavailable || status.Error != ErrHostNotPinned {
		t.Fatalf("status: %+v", status)
	}
	if server.requests.Load() != requests {
		t.Fatal("an unpinned host was contacted")
	}
}

func TestPrepareRejectsInvalidRuleSet(t *testing.T) {
	body := []byte("not a rule set")
	server := newRuleSetServer(t, body)
	manager := newManager(t, server, t.TempDir(), Options{})
	manager.Activate(manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("bad", body)}, []string{server.host()}, true))
	if status := statusOf(t, manager.Statuses(), "bad"); status.State != StateUnavailable || status.Error != ErrInvalid {
		t.Fatalf("status: %+v", status)
	}
}

// A new profile version that cannot be fetched keeps the last good copy.
func TestFailedUpdateKeepsLastGoodCopy(t *testing.T) {
	oldBody := buildSRS(t, []string{"cn.example"}, nil)
	newBody := buildSRS(t, []string{"cn.example", "more.example"}, nil)
	server := newRuleSetServer(t, oldBody)
	dir := t.TempDir()
	manager := newManager(t, server, dir, Options{})
	manager.Activate(manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("cn-site", oldBody)}, []string{server.host()}, true))

	server.set(nil, true)
	snapshot := manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("cn-site", newBody)}, []string{server.host()}, true)
	if file, ok := snapshot.Files()["cn-site"]; !ok || file.Path != filepath.Join(dir, "cn-site.srs") {
		t.Fatalf("stale copy not used: %+v", snapshot.Files())
	}
	manager.Activate(snapshot)
	if status := statusOf(t, manager.Statuses(), "cn-site"); status.State != StateStale || status.Error != ErrHTTPStatus {
		t.Fatalf("status: %+v", status)
	}
	if data, _ := os.ReadFile(filepath.Join(dir, "cn-site.srs")); !bytes.Equal(data, oldBody) {
		t.Fatal("last good copy was replaced")
	}
}

// A ready set is refreshed on its interval with If-None-Match and stays
// ready on 304.
func TestRefreshUsesETag(t *testing.T) {
	body := buildSRS(t, []string{"cn.example"}, nil)
	server := newRuleSetServer(t, body)
	now := &clock{now: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)}
	states := make(chan Status, 8)
	manager := newManager(t, server, t.TempDir(), Options{Now: now.Now, OnState: func(s Status) { states <- s }})
	snapshot := manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("cn-site", body)}, []string{server.host()}, true)
	now.Add(2 * time.Hour)
	manager.Activate(snapshot)
	deadline := time.Now().Add(5 * time.Second)
	for server.notModified.Load() == 0 {
		if time.Now().After(deadline) {
			t.Fatalf("no conditional refresh; requests %d", server.requests.Load())
		}
		time.Sleep(10 * time.Millisecond)
	}
	for {
		status := statusOf(t, manager.Statuses(), "cn-site")
		if status.State == StateReady && status.UpdatedAt != nil && status.UpdatedAt.Equal(now.Now()) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("status after 304: %+v", status)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

// A set that was never downloaded is retried; once it arrives the manager
// asks for a rebuild so the skipped rules take effect.
func TestRecoveryTriggersRebuild(t *testing.T) {
	body := buildSRS(t, nil, []string{"1.0.1.0/24"})
	server := newRuleSetServer(t, body)
	server.set(body, true)
	rebuilds := make(chan struct{}, 4)
	states := make(chan Status, 8)
	manager := newManager(t, server, t.TempDir(), Options{
		RetryMin:  10 * time.Millisecond,
		OnRebuild: func() { rebuilds <- struct{}{} },
		OnState:   func(s Status) { states <- s },
	})
	snapshot := manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("cn-ip", body)}, []string{server.host()}, true)
	if len(snapshot.Files()) != 0 {
		t.Fatalf("files: %+v", snapshot.Files())
	}
	manager.Activate(snapshot)
	if s := <-states; s.State != StateUnavailable || s.Error != ErrHTTPStatus {
		t.Fatalf("first state: %+v", s)
	}
	server.set(body, false)
	select {
	case <-rebuilds:
	case <-time.After(5 * time.Second):
		t.Fatal("no rebuild after recovery")
	}
	if s := <-states; s.State != StateReady {
		t.Fatalf("recovered state: %+v", s)
	}
	// A rebuild prepares again without the network and finds the copy.
	snapshot = manager.Prepare(context.Background(), []profile.RuleSet{server.ruleSet("cn-ip", body)}, []string{server.host()}, false)
	if file, ok := snapshot.Files()["cn-ip"]; !ok || file.MirrorDNS {
		t.Fatalf("files after recovery: %+v", snapshot.Files())
	}
}

func TestInspectClassifiesDNSMirroring(t *testing.T) {
	for name, tc := range map[string]struct {
		suffixes, cidrs []string
		want            bool
	}{
		"domains": {[]string{"cn.example"}, nil, true},
		"cidrs":   {nil, []string{"1.0.1.0/24"}, false},
		"mixed":   {[]string{"cn.example"}, []string{"1.0.1.0/24"}, false},
	} {
		got, err := inspect(buildSRS(t, tc.suffixes, tc.cidrs))
		if err != nil || got != tc.want {
			t.Errorf("%s: %v %v", name, got, err)
		}
	}
}

func TestDownloadsRefuseRedirects(t *testing.T) {
	body := buildSRS(t, []string{"cn.example"}, nil)
	target := newRuleSetServer(t, body)
	redirect := httptest.NewTLSServer(http.RedirectHandler(target.URL+"/x.srs", http.StatusFound))
	t.Cleanup(redirect.Close)
	manager := New(Options{Dir: t.TempDir(), TLSConfig: &tls.Config{RootCAs: redirect.Client().Transport.(*http.Transport).TLSClientConfig.RootCAs}})
	t.Cleanup(manager.Close)
	set := profile.RuleSet{ID: "cn-site", URL: redirect.URL + "/cn-site.srs", SHA256: digest(body)}
	manager.Activate(manager.Prepare(context.Background(), []profile.RuleSet{set}, []string{redirect.Listener.Addr().String()}, true))
	if status := statusOf(t, manager.Statuses(), "cn-site"); status.State != StateUnavailable || status.Error != ErrHTTPStatus || target.requests.Load() != 0 {
		t.Fatalf("status: %+v, target requests %d", status, target.requests.Load())
	}
}
