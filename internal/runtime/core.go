package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/probe"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/routing"
)

type State string

const (
	StateStopped    State = "stopped"
	StateConfigured State = "configured"
	StateRunning    State = "running"
)

type Status struct {
	State          State  `json:"state"`
	Revision       string `json:"revision,omitempty"`
	SelectedNodeID string `json:"selected_node_id,omitempty"`
	NodeCount      int    `json:"node_count"`
	// SelectedIngress is omitted while it is unknown (no profile, or the
	// core is not running).
	SelectedIngress *IngressStatus    `json:"selected_ingress,omitempty"`
	SystemProxy     SystemProxyStatus `json:"system_proxy"`
}

// IngressStatus is the ingress (replica) a node is actually using. For a
// multi-ingress node it is the member that carried the node's latest
// connection (the primary before any traffic); PreviousEndpointKey and
// SwitchedAt describe the latest switch and are omitted before the first one.
type IngressStatus struct {
	EndpointKey         string     `json:"endpoint_key"`
	Label               string     `json:"label,omitempty"`
	PreviousEndpointKey string     `json:"previous_endpoint_key,omitempty"`
	Role                string     `json:"role"`
	SwitchedAt          *time.Time `json:"switched_at,omitempty"`
}

// SystemProxyStatus describes the opt-in unauthenticated loopback listener
// for OS proxy settings. Available reports whether this core can host it;
// Enabled is the host's toggle; Listening is true only while a running
// instance accepts connections on Listen:Port.
type SystemProxyStatus struct {
	Available bool     `json:"available"`
	Enabled   bool     `json:"enabled"`
	Listening bool     `json:"listening"`
	Listen    string   `json:"listen,omitempty"`
	Port      uint16   `json:"port,omitempty"`
	Protocols []string `json:"protocols,omitempty"`
}

var (
	ErrProfileNotApplied  = errors.New("no profile applied")
	ErrLocalProxyDisabled = errors.New("local proxy is disabled for this core")
	ErrCoreNotRunning     = errors.New("core is not running")
	ErrNodeNotFound       = errors.New("node not found")
	// ErrSystemProxyUnavailable: this core cannot host the system proxy (a
	// TUN core, or no private state directory to persist its port).
	ErrSystemProxyUnavailable = errors.New("system proxy is unavailable in this core")
	// ErrSystemProxyStartFailed: the listener could not be opened.
	ErrSystemProxyStartFailed = errors.New("system proxy listener could not be started")
)

// Core serializes lifecycle mutations and owns all sing-box values. Reads and
// event delivery remain concurrent. Profiles are cloned before retention so a
// caller cannot mutate live configuration after validation.
type Core struct {
	operation            sync.RWMutex
	mu                   sync.RWMutex
	active               *profile.Profile
	built                *config.BuildResult
	classifier           *routing.Classifier
	routingGeneration    uint64
	flowAuthorizationKey [32]byte
	flowAuthorizationOK  bool
	platform             profile.PlatformCapabilities
	selected             string
	bus                  *eventBus
	factory              engineFactory
	engine               engine
	cancel               context.CancelFunc
	proxyManager         *localproxy.Manager
	proxyEndpoints       []localproxy.Endpoint
	// The system proxy is only toggled at runtime and always starts
	// disabled; both fields change under the operation lock.
	systemProxyEnabled bool
	systemProxyPort    uint16
}

func New(platform profile.PlatformCapabilities) *Core {
	return newCore(platform, newSingBox)
}

func NewWithLocalProxyState(platform profile.PlatformCapabilities, statePath string) *Core {
	core := newCore(platform, newSingBox)
	core.proxyManager = localproxy.NewManager(statePath)
	return core
}

func newCore(platform profile.PlatformCapabilities, factory engineFactory) *Core {
	key, keyOK := newFlowAuthorizationKey()
	return &Core{
		platform:             platform,
		bus:                  newEventBus(),
		factory:              factory,
		flowAuthorizationKey: key,
		flowAuthorizationOK:  keyOK,
	}
}

func (c *Core) ApplyProfile(p *profile.Profile, now time.Time) (bool, error) {
	c.operation.Lock()
	defer c.operation.Unlock()
	return c.applyProfileLocked(p, now)
}

func (c *Core) applyProfileLocked(p *profile.Profile, now time.Time) (bool, error) {
	c.mu.RLock()
	if c.active != nil && p != nil && c.active.Revision == p.Revision {
		c.mu.RUnlock()
		return false, nil
	}
	oldProfile, oldBuilt, oldInstance, oldInstanceCancel := c.active, c.built, c.engine, c.cancel
	running := oldInstance != nil
	selected := c.selected
	c.mu.RUnlock()

	candidateProfile, err := cloneProfile(p)
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate profile copy failed"})
		return false, err
	}
	if selected != "" && hasNode(candidateProfile, selected) {
		candidateProfile.Selection.DefaultNodeID = selected
	}
	var proxyEndpoints []localproxy.Endpoint
	if c.platform.LocalProxy.Enabled {
		if c.proxyManager == nil {
			return false, fmt.Errorf("local proxy is enabled but no private state path was configured")
		}
		ids := make([]string, len(candidateProfile.Nodes))
		for i, n := range candidateProfile.Nodes {
			ids[i] = n.ID
		}
		if running {
			proxyEndpoints, err = c.proxyManager.Ensure(ids)
		} else {
			proxyEndpoints, err = c.proxyManager.ReconcileForStartup(ids)
		}
		if err != nil {
			return false, fmt.Errorf("prepare local proxies: %w", err)
		}
	}
	candidate, err := config.BuildWithLocalProxies(candidateProfile, c.platform, proxyEndpoints, now)
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate validation or build failed"})
		return false, err
	}
	systemPort := uint16(0)
	if c.systemProxyEnabled {
		// A running listener keeps its port; before start, re-check it.
		if systemPort, err = c.proxyManager.SystemProxyPort(!running, sharedPort(proxyEndpoints)); err != nil {
			return false, fmt.Errorf("prepare system proxy: %w", err)
		}
		candidate = config.WithSystemProxy(candidate, systemPort)
	}
	candidateClassifier, err := routing.Compile(candidateProfile, now)
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate routing compilation failed"})
		return false, err
	}

	var replacement engine
	var replacementCancel context.CancelFunc
	reusePorts := running && len(candidate.Options.Inbounds) > 0
	if running {
		if reusePorts {
			if oldInstanceCancel != nil {
				oldInstanceCancel()
			}
			_ = oldInstance.Close()
		}
		replacement, replacementCancel, err = c.startCandidate(candidate)
		if err != nil {
			if reusePorts && oldBuilt != nil {
				rollback, rollbackCancel, rollbackErr := c.startCandidate(oldBuilt)
				c.mu.Lock()
				c.engine, c.cancel = rollback, rollbackCancel
				c.mu.Unlock()
				if rollbackErr != nil {
					c.mu.Lock()
					c.engine, c.cancel = nil, nil
					c.mu.Unlock()
					c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate start and runtime rollback failed"})
					return false, fmt.Errorf("start candidate: %v; rollback runtime: %w", err, rollbackErr)
				}
			}
			c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate runtime start failed"})
			return false, fmt.Errorf("start candidate runtime: %w", err)
		}
	}

	c.mu.Lock()
	oldEngine, oldCancel := c.engine, c.cancel
	c.active, c.built, c.classifier = candidateProfile, candidate, candidateClassifier
	c.routingGeneration++
	c.proxyEndpoints = proxyEndpoints
	if systemPort != 0 {
		c.systemProxyPort = systemPort
	}
	if c.selected == "" || !hasNode(candidateProfile, c.selected) {
		c.selected = candidateProfile.Selection.DefaultNodeID
	}
	if running {
		c.engine, c.cancel = replacement, replacementCancel
	}
	c.mu.Unlock()

	if oldCancel != nil && running && !reusePorts {
		oldCancel()
	}
	if oldEngine != nil && running && !reusePorts {
		_ = oldEngine.Close()
	}
	if oldProfile != nil {
		for _, n := range candidateProfile.Nodes {
			if before, ok := findNode(oldProfile, n.ID); ok && !sameIngressEndpoints(before, n) {
				c.emit(Event{Type: EventNodeEndpointChanged, At: now, Revision: candidateProfile.Revision, NodeID: n.ID})
			}
		}
	}
	c.emit(Event{Type: EventProfileApplied, At: now, Revision: candidateProfile.Revision})
	return true, nil
}

func (c *Core) LocalProxyEndpoints() []localproxy.Endpoint {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return append([]localproxy.Endpoint(nil), c.proxyEndpoints...)
}

func (c *Core) LocalProxyMetadata() []localproxy.Metadata {
	c.mu.RLock()
	defer c.mu.RUnlock()
	result := make([]localproxy.Metadata, len(c.proxyEndpoints))
	for i, endpoint := range c.proxyEndpoints {
		result[i] = localproxy.Metadata{
			NodeID:       endpoint.NodeID,
			Listen:       endpoint.Listen,
			Port:         endpoint.Port,
			Protocols:    []string{"http", "socks5"},
			AuthRequired: true,
		}
	}
	return result
}

func (c *Core) LocalProxyCredential(nodeID string) (localproxy.Credential, error) {
	if !c.platform.LocalProxy.Enabled {
		return localproxy.Credential{}, ErrLocalProxyDisabled
	}
	c.mu.RLock()
	defer c.mu.RUnlock()
	for _, endpoint := range c.proxyEndpoints {
		if endpoint.NodeID == nodeID {
			return localproxy.Credential{
				NodeID:   endpoint.NodeID,
				Listen:   endpoint.Listen,
				Port:     endpoint.Port,
				Username: endpoint.Username,
				Password: endpoint.Password,
			}, nil
		}
	}
	return localproxy.Credential{}, ErrNodeNotFound
}

func (c *Core) Traffic() Traffic {
	c.mu.RLock()
	instance := c.engine
	c.mu.RUnlock()
	if source, ok := instance.(telemetryEngine); ok {
		traffic, _ := source.telemetrySnapshot()
		return traffic
	}
	return Traffic{MeasuredAt: time.Now()}
}
func (c *Core) Connections() []Connection {
	c.mu.RLock()
	instance, built := c.engine, c.built
	c.mu.RUnlock()
	source, ok := instance.(telemetryEngine)
	if !ok {
		return []Connection{}
	}
	_, connections := source.telemetrySnapshot()
	var reverse map[string]string
	if built != nil {
		reverse = built.OutboundNodes
	}
	for i := range connections {
		connections[i].NodeID = reverse[connections[i].OutboundTag]
	}
	return connections
}

// ProbeEntrances measures every ingress of every node with the given method
// ("tcp" when empty). It needs an applied profile but not a running core.
func (c *Core) ProbeEntrances(ctx context.Context, method probe.Method, timeout time.Duration, concurrency int) ([]probe.EntranceResult, error) {
	return c.probeEntrances(ctx, method, timeout, concurrency, nil)
}

func (c *Core) ProbeEntrancesForNodes(
	ctx context.Context,
	method probe.Method,
	timeout time.Duration,
	concurrency int,
	nodeIDs []string,
) ([]probe.EntranceResult, error) {
	return c.probeEntrances(ctx, method, timeout, concurrency, nodeIDs)
}

func (c *Core) probeEntrances(
	ctx context.Context,
	method probe.Method,
	timeout time.Duration,
	concurrency int,
	nodeIDs []string,
) ([]probe.EntranceResult, error) {
	c.mu.RLock()
	p := c.active
	c.mu.RUnlock()
	if p == nil {
		return nil, ErrProfileNotApplied
	}
	clone, err := cloneProfile(p)
	if err != nil {
		return nil, err
	}
	if len(nodeIDs) > 0 {
		wanted := make(map[string]struct{}, len(nodeIDs))
		for _, id := range nodeIDs {
			wanted[id] = struct{}{}
		}
		nodes := make([]profile.Node, 0, len(nodeIDs))
		for _, node := range clone.Nodes {
			if _, ok := wanted[node.ID]; ok {
				nodes = append(nodes, node)
			}
		}
		if len(nodes) != len(wanted) {
			return nil, fmt.Errorf("one or more probe nodes were not found: %w", ErrNodeNotFound)
		}
		clone.Nodes = nodes
		clone.Selection.DefaultNodeID = nodes[0].ID
		clone.Routing = profile.Routing{Final: profile.RoutingAction{Type: "direct"}}
	}
	results, err := probe.Entrances(ctx, clone, probe.Options{Method: method, Timeout: timeout, Concurrency: concurrency})
	if err == nil {
		for _, result := range results {
			message := result.ErrorCode
			if result.Success {
				message = "success"
			}
			c.emit(Event{Type: EventEntranceProbed, At: result.MeasuredAt, Revision: clone.Revision, NodeID: result.NodeID, Message: message})
		}
	}
	return results, err
}

// ProbeAvailability performs the end-to-end "connect" test through the node's
// authenticated local proxy. It requires a core started with local proxies
// enabled (e.g. `serve --tun=false --local-proxy=true`).
func (c *Core) ProbeAvailability(ctx context.Context, nodeID, target string, timeout time.Duration) (probe.AvailabilityResult, error) {
	if !c.platform.LocalProxy.Enabled {
		return probe.AvailabilityResult{}, ErrLocalProxyDisabled
	}
	c.mu.RLock()
	active, running := c.active != nil, c.engine != nil
	c.mu.RUnlock()
	if !active {
		return probe.AvailabilityResult{}, ErrProfileNotApplied
	}
	if !running {
		return probe.AvailabilityResult{}, ErrCoreNotRunning
	}
	for _, endpoint := range c.LocalProxyEndpoints() {
		if endpoint.NodeID == nodeID {
			result := probe.Availability(ctx, endpoint, target, timeout)
			message := result.ErrorCode
			if result.Success {
				message = "success"
			}
			c.emit(Event{Type: EventAvailabilityProbed, At: result.MeasuredAt, NodeID: nodeID, Message: message})
			return result, nil
		}
	}
	return probe.AvailabilityResult{}, ErrNodeNotFound
}

func (c *Core) Start() error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.RLock()
	if c.engine != nil {
		c.mu.RUnlock()
		return nil
	}
	built, selected, systemPort := c.built, c.selected, c.systemProxyPort
	localPort := sharedPort(c.proxyEndpoints)
	c.mu.RUnlock()
	if built == nil {
		return fmt.Errorf("no profile applied")
	}
	if c.systemProxyEnabled {
		// The port may have been taken while the core was stopped.
		port, err := c.proxyManager.SystemProxyPort(true, localPort)
		if err != nil {
			return fmt.Errorf("prepare system proxy: %w", err)
		}
		if port != systemPort {
			built, systemPort = config.WithSystemProxy(built, port), port
		}
	}
	instance, cancel, err := c.startCandidate(built)
	if err != nil {
		return fmt.Errorf("start runtime: %w", err)
	}
	// The built selector default predates any SelectNode call made while the
	// core was stopped; apply the current selection to the new instance.
	if selector, ok := instance.(flowEngine); ok && selected != "" {
		selector.selectOutbound(built.NodeTags[selected])
	}
	c.mu.Lock()
	c.engine, c.cancel, c.built = instance, cancel, built
	c.systemProxyPort = systemPort
	c.mu.Unlock()
	c.emit(Event{Type: EventCoreStarted, At: time.Now()})
	return nil
}

func (c *Core) Stop() error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.Lock()
	instance, cancel := c.engine, c.cancel
	c.engine, c.cancel = nil, nil
	c.mu.Unlock()
	if instance == nil {
		return nil
	}
	if cancel != nil {
		cancel()
	}
	err := instance.Close()
	c.emit(Event{Type: EventCoreStopped, At: time.Now()})
	return err
}

func (c *Core) Reload() error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.RLock()
	p := c.active
	if p == nil {
		c.mu.RUnlock()
		return fmt.Errorf("no profile applied")
	}
	clone, err := cloneProfile(p)
	originalRevision := p.Revision
	c.mu.RUnlock()
	if err != nil {
		return err
	}
	clone.Revision += "#reload"
	_, err = c.applyProfileLocked(clone, time.Now())
	if err == nil {
		c.mu.Lock()
		c.active.Revision = originalRevision
		c.mu.Unlock()
	}
	return err
}

func (c *Core) startCandidate(candidate *config.BuildResult) (engine, context.CancelFunc, error) {
	ctx, cancel := context.WithCancel(context.Background())
	ctx = failover.WithSwitchObserver(ctx, c.ingressObserver(candidate))
	instance, err := c.factory(ctx, candidate.Options)
	if err != nil {
		cancel()
		return nil, nil, err
	}
	if err = instance.Start(); err != nil {
		cancel()
		_ = instance.Close()
		return nil, nil, err
	}
	return instance, cancel, nil
}

func (c *Core) SelectNode(id string) error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.Lock()
	if c.active == nil || !hasNode(c.active, id) {
		c.mu.Unlock()
		return fmt.Errorf("node not found")
	}
	running, current, built := c.engine, c.active, c.built
	revision := current.Revision
	if running != nil {
		instance, ok := running.(flowEngine)
		if !ok || built == nil || !instance.selectOutbound(built.NodeTags[id]) {
			c.mu.Unlock()
			return fmt.Errorf("runtime does not support node selection")
		}
	}
	c.selected = id
	c.active.Selection.DefaultNodeID = id
	c.mu.Unlock()
	c.emit(Event{Type: EventNodeSelected, At: time.Now(), Revision: revision, NodeID: id})
	return nil
}

func (c *Core) Status() Status {
	c.mu.RLock()
	defer c.mu.RUnlock()
	if c.active == nil {
		return Status{State: StateStopped, SystemProxy: c.systemProxyStatusLocked()}
	}
	state := StateConfigured
	if c.engine != nil {
		state = StateRunning
	}
	return Status{State: state, Revision: c.active.Revision, SelectedNodeID: c.selected, NodeCount: len(c.active.Nodes), SelectedIngress: c.selectedIngressLocked(), SystemProxy: c.systemProxyStatusLocked()}
}

func (c *Core) selectedIngressLocked() *IngressStatus {
	source, ok := c.engine.(ingressEngine)
	if !ok || c.built == nil {
		return nil
	}
	node, ok := findNode(c.active, c.selected)
	if !ok {
		return nil
	}
	active, ok := source.activeIngress(c.built.NodeTags[node.ID])
	if !ok {
		return nil
	}
	key, ok := c.built.IngressKeys[active.Current]
	if !ok {
		return nil
	}
	status := &IngressStatus{EndpointKey: key, PreviousEndpointKey: c.built.IngressKeys[active.Previous]}
	for _, ingress := range node.Ingresses {
		if ingress.EndpointKey == key {
			status.Role, status.Label = string(ingress.Role), ingress.DisplayLabel()
		}
	}
	if !active.SwitchedAt.IsZero() {
		at := active.SwitchedAt.UTC()
		status.SwitchedAt = &at
	}
	return status
}

// SystemProxyAvailable reports whether this core can host the system proxy:
// it needs the private state directory for its port, and a privileged TUN
// core never exposes an unauthenticated listener.
func (c *Core) SystemProxyAvailable() bool {
	return c.proxyManager != nil && !c.platform.TUN.Enabled
}

func (c *Core) SystemProxyStatus() SystemProxyStatus {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.systemProxyStatusLocked()
}

func (c *Core) systemProxyStatusLocked() SystemProxyStatus {
	status := SystemProxyStatus{Available: c.SystemProxyAvailable(), Enabled: c.systemProxyEnabled}
	if c.systemProxyEnabled && c.systemProxyPort != 0 {
		status.Listen, status.Port = localproxy.Listen, c.systemProxyPort
		status.Protocols = []string{"http", "socks5"}
		status.Listening = c.engine != nil
	}
	return status
}

// ingressObserver turns failover switches of the instance built from
// candidate into NodeIngressSwitched events.
func (c *Core) ingressObserver(candidate *config.BuildResult) failover.SwitchObserver {
	return func(group string, active failover.Active) {
		nodeID, ok := candidate.OutboundNodes[group]
		key, known := candidate.IngressKeys[active.Current]
		if !ok || !known {
			return
		}
		c.emit(Event{Type: EventNodeIngressSwitched, At: active.SwitchedAt, NodeID: nodeID, EndpointKey: key, PreviousEndpointKey: candidate.IngressKeys[active.Previous]})
	}
}

// SetSystemProxy opens or closes the unauthenticated loopback system proxy.
// It is idempotent. On a running core the listener is added or removed in
// place, so other connections are not interrupted; otherwise the toggle
// takes effect on the next start. Disabling always closes the listener.
func (c *Core) SetSystemProxy(enabled bool) (SystemProxyStatus, error) {
	c.operation.Lock()
	defer c.operation.Unlock()
	if !c.SystemProxyAvailable() {
		return SystemProxyStatus{}, ErrSystemProxyUnavailable
	}
	if enabled == c.systemProxyEnabled {
		return c.SystemProxyStatus(), nil
	}
	c.mu.RLock()
	instance, built, revision := c.engine, c.built, ""
	localPort := sharedPort(c.proxyEndpoints)
	if c.active != nil {
		revision = c.active.Revision
	}
	c.mu.RUnlock()
	var dynamic inboundEngine
	if instance != nil {
		var ok bool
		if dynamic, ok = instance.(inboundEngine); !ok {
			return SystemProxyStatus{}, ErrSystemProxyUnavailable
		}
	}
	port := uint16(0)
	if enabled {
		var err error
		if port, err = c.proxyManager.SystemProxyPort(true, localPort); err != nil {
			return SystemProxyStatus{}, fmt.Errorf("%w: %v", ErrSystemProxyStartFailed, err)
		}
		if dynamic != nil {
			if err = dynamic.addInbound(config.SystemProxyInbound(port)); err != nil {
				_ = dynamic.removeInbound(config.SystemProxyInboundTag)
				return SystemProxyStatus{}, fmt.Errorf("%w: %v", ErrSystemProxyStartFailed, err)
			}
		}
	} else if dynamic != nil {
		if err := dynamic.removeInbound(config.SystemProxyInboundTag); err != nil {
			return SystemProxyStatus{}, fmt.Errorf("close system proxy: %w", err)
		}
	}
	c.mu.Lock()
	c.systemProxyEnabled = enabled
	if enabled {
		c.systemProxyPort = port
	}
	if built != nil {
		c.built = config.WithSystemProxy(built, port)
	}
	status := c.systemProxyStatusLocked()
	c.mu.Unlock()
	message := "disabled"
	if enabled {
		message = "enabled"
	}
	c.emit(Event{Type: EventSystemProxyChanged, At: time.Now(), Revision: revision, Message: message})
	return status, nil
}

func sharedPort(endpoints []localproxy.Endpoint) uint16 {
	if len(endpoints) == 0 {
		return 0
	}
	return endpoints[0].Port
}

func (c *Core) Nodes() []profile.Node {
	c.mu.RLock()
	defer c.mu.RUnlock()
	if c.active == nil {
		return nil
	}
	return append([]profile.Node(nil), c.active.Nodes...)
}

func (c *Core) Subscribe(ctx context.Context, buffer int) <-chan Event {
	if buffer < 1 {
		buffer = 1
	}
	ch := make(chan Event, buffer)
	c.mu.Lock()
	id := c.bus.next
	c.bus.next++
	c.bus.subscribers[id] = ch
	c.mu.Unlock()
	go func() {
		<-ctx.Done()
		c.mu.Lock()
		delete(c.bus.subscribers, id)
		close(ch)
		c.mu.Unlock()
	}()
	return ch
}

func (c *Core) emit(e Event) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	for _, ch := range c.bus.subscribers {
		select {
		case ch <- e:
		default:
		}
	}
}

func cloneProfile(p *profile.Profile) (*profile.Profile, error) {
	if p == nil {
		return nil, fmt.Errorf("profile is required")
	}
	data, err := json.Marshal(p)
	if err != nil {
		return nil, err
	}
	var clone profile.Profile
	if err = json.Unmarshal(data, &clone); err != nil {
		return nil, err
	}
	return &clone, nil
}

// LocalProxyEnabled reports whether this core runs the shared local proxy.
func (c *Core) LocalProxyEnabled() bool { return c.platform.LocalProxy.Enabled }

func sameIngressEndpoints(a, b profile.Node) bool {
	if len(a.Ingresses) != len(b.Ingresses) {
		return false
	}
	for i := range a.Ingresses {
		if a.Ingresses[i].Endpoint != b.Ingresses[i].Endpoint || a.Ingresses[i].EndpointKey != b.Ingresses[i].EndpointKey {
			return false
		}
	}
	return true
}

func hasNode(p *profile.Profile, id string) bool { _, ok := findNode(p, id); return ok }
func findNode(p *profile.Profile, id string) (profile.Node, bool) {
	for _, n := range p.Nodes {
		if n.ID == id {
			return n, true
		}
	}
	return profile.Node{}, false
}
