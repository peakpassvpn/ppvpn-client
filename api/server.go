package api

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	coreruntime "github.com/peakpassvpn/ppvpn-core/internal/runtime"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/version"
)

const MaxRequestBytes = 4 << 20

type Server struct {
	core   *coreruntime.Core
	secret string
	mux    *http.ServeMux
}

func NewServer(core *coreruntime.Core, sessionSecret string) (*Server, error) {
	if core == nil {
		return nil, fmt.Errorf("core is required")
	}
	if len(sessionSecret) < 32 {
		return nil, fmt.Errorf("session secret must contain at least 32 characters")
	}
	s := &Server{core: core, secret: sessionSecret, mux: http.NewServeMux()}
	s.routes()
	return s, nil
}
func (s *Server) Handler() http.Handler { return s.authenticate(s.mux) }

func (s *Server) routes() {
	s.mux.HandleFunc("POST /v1/get-version", s.simple(func(_ *http.Request) (any, error) { return version.Get(), nil }))
	s.mux.HandleFunc("POST /v1/validate-profile", s.validateProfile)
	s.mux.HandleFunc("POST /v1/apply-profile", s.applyProfile)
	s.mux.HandleFunc("POST /v1/start", s.simple(func(_ *http.Request) (any, error) { return map[string]any{}, s.core.Start() }))
	s.mux.HandleFunc("POST /v1/stop", s.simple(func(_ *http.Request) (any, error) { return map[string]any{}, s.core.Stop() }))
	s.mux.HandleFunc("POST /v1/reload", s.simple(func(_ *http.Request) (any, error) { return map[string]any{}, s.core.Reload() }))
	s.mux.HandleFunc("POST /v1/get-status", s.simple(func(_ *http.Request) (any, error) { return s.core.Status(), nil }))
	s.mux.HandleFunc("POST /v1/list-nodes", s.simple(func(_ *http.Request) (any, error) {
		nodes := s.core.Nodes()
		out := make([]NodeSummary, len(nodes))
		for i, n := range nodes {
			out[i] = summarize(n)
		}
		return out, nil
	}))
	s.mux.HandleFunc("POST /v1/select-node", s.selectNode)
	s.mux.HandleFunc("POST /v1/get-selected-node", s.simple(func(_ *http.Request) (any, error) {
		status := s.core.Status()
		for _, n := range s.core.Nodes() {
			if n.ID == status.SelectedNodeID {
				return summarize(n), nil
			}
		}
		return nil, apiError("NODE_NOT_FOUND", "selected node not found", "", false)
	}))
	s.mux.HandleFunc("POST /v1/probe-entrances", s.probeEntrances)
	s.mux.HandleFunc("POST /v1/probe-availability", s.probeAvailability)
	s.mux.HandleFunc("POST /v1/get-local-proxy-metadata", s.simple(func(_ *http.Request) (any, error) {
		return s.core.LocalProxyMetadata(), nil
	}))
	s.mux.HandleFunc("POST /v1/get-local-proxy-credential", s.localProxyCredential)
	// Retained for Core API v1 compatibility. New hosts should use the
	// metadata and per-node credential methods so secrets never enter general
	// UI state.
	s.mux.HandleFunc("POST /v1/get-local-proxy-endpoints", s.simple(func(_ *http.Request) (any, error) { return s.core.LocalProxyEndpoints(), nil }))
	s.mux.HandleFunc("POST /v1/get-system-proxy-endpoints", s.simple(func(_ *http.Request) (any, error) {
		endpoints, err := s.core.SystemProxyEndpoints()
		if errors.Is(err, coreruntime.ErrSystemProxyUnavailable) {
			return nil, apiError("SYSTEM_PROXY_UNAVAILABLE", "system proxy capability is unavailable", "", false)
		}
		if errors.Is(err, coreruntime.ErrProfileNotApplied) {
			return nil, apiError("PROFILE_NOT_APPLIED", "no profile has been applied", "", false)
		}
		return endpoints, err
	}))
	s.mux.HandleFunc("POST /v1/get-traffic", s.simple(func(_ *http.Request) (any, error) { return s.core.Traffic(), nil }))
	s.mux.HandleFunc("POST /v1/get-connections", s.simple(func(_ *http.Request) (any, error) { return s.core.Connections(), nil }))
	s.mux.HandleFunc("GET /v1/watch-events", s.watchEvents)
	s.mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		write(w, http.StatusNotFound, Envelope{RequestID: requestID(r), OK: false, Error: &Error{Code: "API_NOT_FOUND", Message: "Core API method was not found"}})
	})
}

func (s *Server) authenticate(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		provided := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
		if len(provided) != len(s.secret) || subtle.ConstantTimeCompare([]byte(provided), []byte(s.secret)) != 1 {
			write(w, http.StatusUnauthorized, Envelope{RequestID: requestID(r), OK: false, Error: &Error{Code: "UNAUTHENTICATED", Message: "valid session authentication is required"}})
			return
		}
		if requested := r.Header.Get("X-Core-API-Version"); requested != "" && requested != "1" {
			write(w, http.StatusBadRequest, Envelope{RequestID: requestID(r), OK: false, Error: &Error{Code: "CORE_API_UNSUPPORTED", Message: "requested Core API version is unsupported"}})
			return
		}
		next.ServeHTTP(w, r)
	})
}
func (s *Server) simple(fn func(*http.Request) (any, error)) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) { data, err := fn(r); s.respond(w, r, data, err) }
}
func (s *Server) validateProfile(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	p, err := profile.Parse(request.Profile)
	if err == nil {
		err = profile.Validate(p, time.Now())
	}
	s.respond(w, r, map[string]bool{"valid": err == nil}, err)
}
func (s *Server) applyProfile(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	p, err := profile.Parse(request.Profile)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	applied, err := s.core.ApplyProfile(p, time.Now())
	s.respond(w, r, map[string]bool{"applied": applied}, err)
}
func (s *Server) selectNode(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err == nil {
		err = s.core.SelectNode(request.NodeID)
	}
	s.respond(w, r, map[string]string{"node_id": request.NodeID}, err)
}
func (s *Server) probeEntrances(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	timeout := duration(request.TimeoutMS, 5*time.Second)
	var result any
	if len(request.NodeIDs) > 0 {
		result, err = s.core.ProbeEntrancesForNodes(
			r.Context(),
			timeout,
			request.Concurrency,
			request.NodeIDs,
		)
	} else {
		result, err = s.core.ProbeEntrances(r.Context(), timeout, request.Concurrency)
	}
	s.respond(w, r, result, err)
}
func (s *Server) probeAvailability(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	result, err := s.core.ProbeAvailability(r.Context(), request.NodeID, request.Target, duration(request.TimeoutMS, 10*time.Second))
	if err != nil {
		s.respond(w, r, nil, apiError("NODE_NOT_FOUND", "node local proxy endpoint not found", "node_id", false))
		return
	}
	s.respond(w, r, result, nil)
}
func (s *Server) localProxyCredential(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	credential, err := s.core.LocalProxyCredential(request.NodeID)
	if err != nil {
		s.respond(w, r, nil, apiError("NODE_NOT_FOUND", "node local proxy endpoint not found", "node_id", false))
		return
	}
	s.respond(w, r, credential, nil)
}
func (s *Server) watchEvents(w http.ResponseWriter, r *http.Request) {
	flusher, ok := w.(http.Flusher)
	if !ok {
		s.respond(w, r, nil, apiError("STREAM_UNSUPPORTED", "event streaming is unavailable", "", false))
		return
	}
	w.Header().Set("Content-Type", "application/x-ndjson")
	w.WriteHeader(http.StatusOK)
	events := s.core.Subscribe(r.Context(), 64)
	encoder := json.NewEncoder(w)
	id := requestID(r)
	for event := range events {
		if encoder.Encode(Envelope{RequestID: id, OK: true, Data: event}) != nil {
			return
		}
		flusher.Flush()
	}
}

func (s *Server) respond(w http.ResponseWriter, r *http.Request, data any, err error) {
	if err == nil {
		write(w, http.StatusOK, Envelope{RequestID: requestID(r), OK: true, Data: data})
		return
	}
	var structured *profile.ValidationError
	if errors.As(err, &structured) {
		write(w, http.StatusBadRequest, Envelope{RequestID: requestID(r), OK: false, Error: &Error{Code: structured.Code, Message: structured.Message, Field: structured.Field, Retryable: structured.Retryable}})
		return
	}
	var ae *apiErr
	if errors.As(err, &ae) {
		write(w, http.StatusBadRequest, Envelope{RequestID: requestID(r), OK: false, Error: &ae.Detail})
		return
	}
	write(w, http.StatusBadRequest, Envelope{RequestID: requestID(r), OK: false, Error: &Error{Code: "CORE_OPERATION_FAILED", Message: "core operation failed"}})
}
func decode(r *http.Request) (rawRequest, error) {
	defer r.Body.Close()
	decoder := json.NewDecoder(io.LimitReader(r.Body, MaxRequestBytes+1))
	decoder.DisallowUnknownFields()
	var request rawRequest
	if err := decoder.Decode(&request); err != nil {
		return request, apiError("REQUEST_INVALID", "request body is invalid", "", false)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		return request, apiError("REQUEST_INVALID", "request body must contain exactly one JSON value", "", false)
	}
	return request, nil
}
func write(w http.ResponseWriter, status int, envelope Envelope) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(envelope)
}
func requestID(r *http.Request) string {
	if id := r.Header.Get("X-Request-ID"); id != "" && len(id) <= 128 {
		return id
	}
	bytes := make([]byte, 12)
	if _, err := rand.Read(bytes); err != nil {
		return "unknown"
	}
	return hex.EncodeToString(bytes)
}
func duration(ms int, fallback time.Duration) time.Duration {
	if ms <= 0 {
		return fallback
	}
	if ms > 120000 {
		ms = 120000
	}
	return time.Duration(ms) * time.Millisecond
}
func summarize(n profile.Node) NodeSummary {
	summary := NodeSummary{ID: n.ID, Name: n.Name, Protocol: string(n.Protocol), TCP: n.Capabilities.TCP, UDP: n.Capabilities.UDP}
	if n.Exit != nil {
		summary.Region = n.Exit.Region
		summary.CountryCode = n.Exit.CountryCode
	}
	return summary
}

type apiErr struct{ Detail Error }

func (e *apiErr) Error() string { return e.Detail.Message }
func apiError(code, message, field string, retryable bool) error {
	return &apiErr{Error{Code: code, Message: message, Field: field, Retryable: retryable}}
}
