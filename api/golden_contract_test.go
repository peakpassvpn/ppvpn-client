package api

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/goldentest"
	coreruntime "github.com/peakpassvpn/ppvpn-core/internal/runtime"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

// The contract golden files (testdata/golden/contract) are the behaviour
// baseline the Rust engine is checked against: language-independent inputs
// (Core API v1 methods and request bodies, profiles as a base file plus an
// RFC 6902 JSON Patch) and the responses and events Go produces. See
// testdata/golden/README.md. go test -run TestGoldenContract -update
// rewrites the "expect" parts.
var updateGolden = flag.Bool("update", false, "rewrite the golden files' expectations from the current behaviour")

const contractDir = "../testdata/golden/contract"

// contractFile is one golden file: a sequence of steps against one core.
type contractFile struct {
	Name        string            `json:"name"`
	Description string            `json:"description"`
	Platform    json.RawMessage   `json:"platform,omitempty"`
	Steps       []contractStep    `json:"steps"`
	Expect      []json.RawMessage `json:"expect,omitempty"`
}

// contractStep calls one Core API v1 method. ProfileRef, when set, becomes
// the request body's "profile": the base file with the patch applied.
type contractStep struct {
	Name       string                 `json:"name,omitempty"`
	Method     string                 `json:"method"`
	Body       json.RawMessage        `json:"body,omitempty"`
	ProfileRef *goldentest.ProfileRef `json:"profile_ref,omitempty"`
}

// stepResult is what a step produced: the response envelope without
// request_id and error.message (free text, not contract), and the events
// emitted during the step without their "at".
type stepResult struct {
	Status   int             `json:"status"`
	Response json.RawMessage `json:"response"`
	Events   []any           `json:"events"`
}

func TestGoldenContract(t *testing.T) {
	files, err := filepath.Glob(filepath.Join(contractDir, "*.json"))
	if err != nil || len(files) == 0 {
		t.Fatalf("no contract files in %s: %v", contractDir, err)
	}
	for _, path := range files {
		t.Run(strings.TrimSuffix(filepath.Base(path), ".json"), func(t *testing.T) {
			raw, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			var file contractFile
			if err := json.Unmarshal(raw, &file); err != nil {
				t.Fatal(err)
			}
			results := runContract(t, file)
			if *updateGolden {
				file.Expect = results
				out, err := marshalGolden(file)
				if err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(path, out, 0o644); err != nil {
					t.Fatal(err)
				}
				return
			}
			if len(file.Expect) != len(results) {
				t.Fatalf("%d expectations for %d steps; rerun with -update", len(file.Expect), len(results))
			}
			for i := range results {
				if !jsonEqual(file.Expect[i], results[i]) {
					t.Errorf("step %d (%s):\n got %s\nwant %s", i, file.Steps[i].Method, results[i], file.Expect[i])
				}
			}
		})
	}
}

func runContract(t *testing.T, file contractFile) []json.RawMessage {
	t.Helper()
	platform := profile.PlatformCapabilities{}
	if len(file.Platform) > 0 {
		if err := json.Unmarshal(file.Platform, &platform); err != nil {
			t.Fatalf("platform: %v", err)
		}
	}
	core := coreruntime.New(platform)
	server, err := NewServer(core, testSecret)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = core.Stop() })
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	events := core.Subscribe(ctx, 256)
	var results []json.RawMessage
	for i, step := range file.Steps {
		body, err := stepBody(step)
		if err != nil {
			t.Fatalf("step %d: %v", i, err)
		}
		req := httptest.NewRequest(http.MethodPost, "/v1/"+step.Method, bytes.NewReader(body))
		req.Header.Set("Authorization", "Bearer "+testSecret)
		req.Header.Set("X-Core-API-Version", "1")
		req.Header.Set("X-Request-ID", "step-"+strconv.Itoa(i))
		rec := httptest.NewRecorder()
		server.Handler().ServeHTTP(rec, req)
		result := stepResult{Status: rec.Code, Response: normalizeResponse(t, rec.Body.Bytes()), Events: drainEvents(t, events)}
		out, err := json.Marshal(result)
		if err != nil {
			t.Fatal(err)
		}
		results = append(results, out)
	}
	return results
}

func stepBody(step contractStep) ([]byte, error) {
	body := map[string]json.RawMessage{}
	if len(step.Body) > 0 {
		if err := json.Unmarshal(step.Body, &body); err != nil {
			return nil, err
		}
	}
	if step.ProfileRef != nil {
		profileJSON, err := goldentest.ResolveProfile(contractDir, *step.ProfileRef)
		if err != nil {
			return nil, err
		}
		body["profile"] = profileJSON
	}
	if len(body) == 0 && len(step.Body) == 0 {
		return nil, nil
	}
	return json.Marshal(body)
}

// normalizeResponse drops what is not contract or not deterministic:
// request_id, error.message and every timestamp (keys "at" or "*_at").
func normalizeResponse(t *testing.T, body []byte) json.RawMessage {
	t.Helper()
	var envelope map[string]any
	if err := json.Unmarshal(body, &envelope); err != nil {
		t.Fatalf("response %q: %v", body, err)
	}
	delete(envelope, "request_id")
	if e, ok := envelope["error"].(map[string]any); ok {
		delete(e, "message")
	}
	out, _ := json.Marshal(scrubTimes(envelope))
	return out
}

func scrubTimes(v any) any {
	switch value := v.(type) {
	case map[string]any:
		for key, child := range value {
			if key == "at" || strings.HasSuffix(key, "_at") {
				if child != nil && child != "" {
					value[key] = "<time>"
				}
				continue
			}
			value[key] = scrubTimes(child)
		}
	case []any:
		for i := range value {
			value[i] = scrubTimes(value[i])
		}
	}
	return v
}

// drainEvents collects the events a step emitted: the core emits them before
// the call returns, so whatever arrives within a short quiet period.
func drainEvents(t *testing.T, events <-chan coreruntime.Event) []any {
	t.Helper()
	out := []any{}
	for {
		select {
		case event := <-events:
			data, err := json.Marshal(event)
			if err != nil {
				t.Fatal(err)
			}
			var decoded map[string]any
			_ = json.Unmarshal(data, &decoded)
			delete(decoded, "at")
			out = append(out, scrubTimes(decoded))
		case <-time.After(30 * time.Millisecond):
			return out
		}
	}
}

func jsonEqual(a, b json.RawMessage) bool {
	var x, y any
	if json.Unmarshal(a, &x) != nil || json.Unmarshal(b, &y) != nil {
		return false
	}
	ax, _ := json.Marshal(x)
	by, _ := json.Marshal(y)
	return bytes.Equal(ax, by)
}

// marshalGolden writes a file with stable key order and one expectation per
// step, readable in a diff.
func marshalGolden(file contractFile) ([]byte, error) {
	for i, e := range file.Expect {
		var v any
		if err := json.Unmarshal(e, &v); err != nil {
			return nil, err
		}
		canonical, err := json.Marshal(v) // encoding/json sorts map keys
		if err != nil {
			return nil, err
		}
		file.Expect[i] = canonical
	}
	var buf bytes.Buffer
	encoder := json.NewEncoder(&buf)
	encoder.SetEscapeHTML(false)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(file); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}
