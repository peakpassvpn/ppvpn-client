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
	"runtime/pprof"
	"strings"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	coreruntime "github.com/peakpassvpn/ppvpn-core/internal/runtime"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/probe"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/version"
)

const MaxRequestBytes = 4 << 20

type Server struct {
	core   *coreruntime.Core
	secret string
	mux    *http.ServeMux
	log    *corelog.Logger
}

// SetLogger sets where the causes of folded errors are logged.
func (s *Server) SetLogger(log *corelog.Logger) { s.log = log }

func NewServer(core *coreruntime.Core, sessionSecret string) (*Server, error) {
	if core == nil {
		return nil, fmt.Errorf("core is required")
	}
	if len(sessionSecret) < 32 {
		return nil, fmt.Errorf("session secret must contain at least 32 characters")
	}
	s := &Server{core: core, secret: sessionSecret, mux: http.NewServeMux(), log: corelog.Discard()}
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
	s.mux.HandleFunc("POST /v1/pin-ingress", s.pinIngress)
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
		if !s.core.LocalProxyEnabled() {
			return nil, coreError(coreruntime.ErrLocalProxyDisabled)
		}
		return s.core.LocalProxyMetadata(), nil
	}))
	s.mux.HandleFunc("POST /v1/get-local-proxy-credential", s.localProxyCredential)
	// Retained for Core API v1 compatibility. New hosts should use the
	// metadata and per-node credential methods so secrets never enter general
	// UI state.
	s.mux.HandleFunc("POST /v1/get-local-proxy-endpoints", s.simple(func(_ *http.Request) (any, error) {
		if !s.core.LocalProxyEnabled() {
			return nil, coreError(coreruntime.ErrLocalProxyDisabled)
		}
		return s.core.LocalProxyEndpoints(), nil
	}))
	// Opt-in unauthenticated loopback proxy for OS proxy settings. It is off
	// whenever the core starts; its state is also part of get-status.
	s.mux.HandleFunc("POST /v1/set-system-proxy", s.setSystemProxy)
	s.mux.HandleFunc("POST /v1/get-system-proxy-endpoints", s.simple(func(_ *http.Request) (any, error) {
		if !s.core.SystemProxyAvailable() {
			return nil, coreError(coreruntime.ErrSystemProxyUnavailable)
		}
		return s.core.SystemProxyStatus(), nil
	}))
	s.mux.HandleFunc("POST /v1/get-traffic", s.simple(func(_ *http.Request) (any, error) { return s.core.Traffic(), nil }))
	s.mux.HandleFunc("POST /v1/get-connections", s.simple(func(_ *http.Request) (any, error) { return s.core.Connections(), nil }))
	s.mux.HandleFunc("GET /v1/watch-events", s.watchEvents)
	s.mux.HandleFunc("GET /v1/debug/goroutines", s.debugGoroutines)
	s.mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		write(w, http.StatusNotFound, Envelope{RequestID: requestID(r), OK: false, Error: &Error{Code: "API_NOT_FOUND", Message: "Core API method was not found"}})
	})
}

// debugGoroutines returns every goroutine's stack (pprof goroutine, debug=2)
// as text, to see where a core is stuck on platforms without SIGQUIT. It
// exists only while the core logs at debug level; otherwise it answers like
// an unknown method. Authentication applies as for every method.
func (s *Server) debugGoroutines(w http.ResponseWriter, r *http.Request) {
	if !s.log.DebugEnabled() {
		write(w, http.StatusNotFound, Envelope{RequestID: requestID(r), OK: false, Error: &Error{Code: "API_NOT_FOUND", Message: "Core API method was not found"}})
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_ = pprof.Lookup("goroutine").WriteTo(w, 2)
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
	if _, modeErr := routingMode(request); modeErr != nil {
		s.respond(w, r, nil, modeErr)
		return
	}
	p, err := profile.Parse(request.Profile)
	if err == nil {
		err = profile.Validate(p, time.Now())
	}
	if err == nil && len(request.AllowedRuleSetHosts) > 0 {
		err = profile.ValidateRuleSetHosts(p, request.AllowedRuleSetHosts)
	}
	s.respond(w, r, map[string]bool{"valid": err == nil}, err)
}
func (s *Server) applyProfile(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	mode, err := routingMode(request)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	p, err := profile.Parse(request.Profile)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	applied, err := s.core.ApplyProfileWithOptions(p, time.Now(), coreruntime.ApplyOptions{AllowedRuleSetHosts: request.AllowedRuleSetHosts, RoutingMode: mode})
	s.respond(w, r, map[string]bool{"applied": applied}, err)
}
func (s *Server) selectNode(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err == nil {
		err = s.core.SelectNode(request.NodeID)
	}
	s.respond(w, r, map[string]string{"node_id": request.NodeID}, err)
}

// pinIngress pins a node to one ingress ({"node_id", "endpoint_key"}), or
// returns it to automatic failover ("endpoint_key": null).
func (s *Server) pinIngress(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	key := ""
	if err == nil && request.EndpointKey != nil {
		if key = *request.EndpointKey; key == "" {
			err = apiError("INGRESS_NOT_FOUND", "endpoint_key must be an ingress endpoint_key or null", "endpoint_key", false)
		}
	}
	if err == nil {
		err = coreError(s.core.PinIngress(request.NodeID, key))
	}
	s.respond(w, r, map[string]any{"node_id": request.NodeID, "endpoint_key": request.EndpointKey}, err)
}

func (s *Server) probeEntrances(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	method, err := probe.ParseMethod(request.Method)
	if err != nil {
		s.respond(w, r, nil, apiError("PROBE_METHOD_UNSUPPORTED", "probe method must be tcp or icmp", "method", false))
		return
	}
	timeout := duration(request.TimeoutMS, 5*time.Second)
	var result any
	if len(request.NodeIDs) > 0 {
		result, err = s.core.ProbeEntrancesForNodes(
			r.Context(),
			method,
			timeout,
			request.Concurrency,
			request.NodeIDs,
		)
	} else {
		result, err = s.core.ProbeEntrances(r.Context(), method, timeout, request.Concurrency)
	}
	s.respond(w, r, result, coreError(err))
}
func (s *Server) probeAvailability(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	result, err := s.core.ProbeAvailability(r.Context(), request.NodeID, request.Target, duration(request.TimeoutMS, 10*time.Second))
	if err != nil {
		s.respond(w, r, nil, coreError(err))
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
	var credential localproxy.Credential
	switch request.Kind {
	case "", localproxy.KindNode:
		credential, err = s.core.LocalProxyCredential(request.NodeID)
	case localproxy.KindRouted:
		if request.NodeID != "" {
			s.respond(w, r, nil, apiError("REQUEST_INVALID", "node_id must be empty for kind routed", "node_id", false))
			return
		}
		credential, err = s.core.LocalProxyRoutedCredential()
	default:
		s.respond(w, r, nil, apiError("REQUEST_INVALID", "kind must be node or routed", "kind", false))
		return
	}
	if err != nil {
		s.respond(w, r, nil, coreError(err))
		return
	}
	s.respond(w, r, credential, nil)
}
func (s *Server) setSystemProxy(w http.ResponseWriter, r *http.Request) {
	request, err := decode(r)
	if err != nil {
		s.respond(w, r, nil, err)
		return
	}
	if request.Enabled == nil {
		s.respond(w, r, nil, apiError("REQUEST_INVALID", "enabled is required", "enabled", false))
		return
	}
	status, err := s.core.SetSystemProxy(*request.Enabled)
	if err != nil {
		s.respond(w, r, nil, coreError(err))
		return
	}
	s.respond(w, r, status, nil)
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

// lifecyclePaths are logged on success too, so the log shows the sequence
// that led to a failure.
var lifecyclePaths = map[string]bool{"/v1/apply-profile": true, "/v1/start": true, "/v1/stop": true, "/v1/reload": true, "/v1/set-system-proxy": true, "/v1/pin-ingress": true}

func (s *Server) respond(w http.ResponseWriter, r *http.Request, data any, err error) {
	id := requestID(r)
	if err == nil {
		if lifecyclePaths[r.URL.Path] {
			s.log.Info("request ok", "path", r.URL.Path, "request_id", id)
		}
		write(w, http.StatusOK, Envelope{RequestID: id, OK: true, Data: data})
		return
	}
	var structured *profile.ValidationError
	if errors.As(err, &structured) {
		s.log.Info("request rejected", "path", r.URL.Path, "request_id", id, "code", structured.Code, "field", structured.Field, "stage", coreruntime.Stages(err))
		write(w, http.StatusBadRequest, Envelope{RequestID: id, OK: false, Error: &Error{Code: structured.Code, Message: structured.Message, Field: structured.Field, Retryable: structured.Retryable}})
		return
	}
	var ae *apiErr
	if errors.As(err, &ae) {
		s.log.Info("request rejected", "path", r.URL.Path, "request_id", id, "code", ae.Detail.Code)
		write(w, http.StatusBadRequest, Envelope{RequestID: id, OK: false, Error: &ae.Detail})
		return
	}
	// The response stays folded; the cause, its stage and wrap chain go to
	// the core log only.
	s.log.Error("CORE_OPERATION_FAILED", "path", r.URL.Path, "request_id", id, "stage", coreruntime.Stages(err), "error", err, "chain", corelog.Chain(err))
	write(w, http.StatusBadRequest, Envelope{RequestID: id, OK: false, Error: &Error{Code: "CORE_OPERATION_FAILED", Message: "core operation failed"}})
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
	summary := NodeSummary{ID: n.ID, Name: n.Name, EntryKey: n.EntryKey, EntryLabel: n.EntryLabel, Region: n.Exit.Region, TCP: n.Capabilities.TCP, UDP: n.Capabilities.UDP, Ingresses: make([]IngressSummary, len(n.Ingresses))}
	for i, ingress := range n.Ingresses {
		summary.Ingresses[i] = IngressSummary{EndpointKey: ingress.EndpointKey, Label: ingress.DisplayLabel(), ReplicaOrdinal: ingress.ReplicaOrdinal, Role: string(ingress.Role), Protocol: string(ingress.Protocol)}
	}
	if len(n.Ingresses) > 0 {
		summary.Protocol = string(n.Ingresses[0].Protocol)
	}
	return summary
}

// coreError maps runtime sentinel errors to stable API error codes.
func coreError(err error) error {
	switch {
	case err == nil:
		return nil
	case errors.Is(err, coreruntime.ErrLocalProxyDisabled):
		return apiError("LOCAL_PROXY_DISABLED", "this core was started without local proxies (--local-proxy=false)", "", false)
	case errors.Is(err, coreruntime.ErrCoreNotRunning):
		return apiError("CORE_NOT_RUNNING", "the core must be started before local proxies accept connections", "", true)
	case errors.Is(err, coreruntime.ErrProfileNotApplied):
		return apiError("PROFILE_NOT_APPLIED", "no profile has been applied", "", false)
	case errors.Is(err, coreruntime.ErrSystemProxyUnavailable):
		return apiError("SYSTEM_PROXY_UNAVAILABLE", "this core cannot host the system proxy (TUN core or no state directory)", "", false)
	case errors.Is(err, coreruntime.ErrSystemProxyStartFailed):
		return apiError("SYSTEM_PROXY_START_FAILED", "the system proxy listener could not be opened", "", true)
	case errors.Is(err, coreruntime.ErrNodeNotFound):
		return apiError("NODE_NOT_FOUND", "node not found", "node_id", false)
	case errors.Is(err, coreruntime.ErrNoDefaultInterface):
		return apiError("NO_DEFAULT_INTERFACE", "the host has no default network interface; probe again once the network is back", "", true)
	case errors.Is(err, coreruntime.ErrIngressNotFound):
		return apiError("INGRESS_NOT_FOUND", "the node has no ingress with this endpoint_key", "endpoint_key", false)
	default:
		return err
	}
}

type apiErr struct{ Detail Error }

func (e *apiErr) Error() string { return e.Detail.Message }

// routingMode reads the optional routing_mode of apply/validate-profile.
func routingMode(request rawRequest) (coreruntime.RoutingMode, error) {
	mode, err := coreruntime.ParseRoutingMode(request.RoutingMode)
	if err != nil {
		return "", apiError("ROUTING_MODE_INVALID", "routing_mode must be rules or global", "routing_mode", false)
	}
	return mode, nil
}

func apiError(code, message, field string, retryable bool) error {
	return &apiErr{Error{Code: code, Message: message, Field: field, Retryable: retryable}}
}
