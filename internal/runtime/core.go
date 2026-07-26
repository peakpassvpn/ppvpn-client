package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/probe"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/routing"
	"github.com/peakpassvpn/ppvpn-core/systemproxy"
)

type State string

const (
	StateStopped    State = "stopped"
	StateConfigured State = "configured"
	StateRunning    State = "running"
)

type Status struct {
	State          State             `json:"state"`
	Revision       string            `json:"revision,omitempty"`
	SelectedNodeID string            `json:"selected_node_id,omitempty"`
	NodeCount      int               `json:"node_count"`
	SystemProxy    SystemProxyStatus `json:"system_proxy"`
}

type SystemProxyStatus struct {
	Enabled   bool                   `json:"enabled"`
	Available bool                   `json:"available"`
	Endpoints *systemproxy.Endpoints `json:"endpoints,omitempty"`
}

var (
	ErrSystemProxyUnavailable = errors.New("system proxy capability is unavailable")
	ErrProfileNotApplied      = errors.New("no profile applied")
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
	systemProxyManager   *systemproxy.Manager
	systemProxyEndpoint  *systemproxy.Endpoint
}

func New(platform profile.PlatformCapabilities) *Core {
	return newCore(platform, newSingBox)
}

func NewWithLocalProxyState(platform profile.PlatformCapabilities, statePath string) *Core {
	core := newCore(platform, newSingBox)
	core.proxyManager = localproxy.NewManager(statePath)
	core.systemProxyManager = systemproxy.NewManager(filepath.Join(filepath.Dir(statePath), "system-proxy.json"))
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
	var systemEndpoint *systemproxy.Endpoint
	endpointChanged := false
	if c.platform.SystemProxy.Enabled {
		if err = systemproxy.ValidateListen(c.platform.SystemProxy.Listen); err != nil {
			return false, err
		}
		if c.systemProxyManager == nil {
			return false, fmt.Errorf("system proxy is enabled but no private state path was configured")
		}
		reserved := make(map[uint16]bool, len(proxyEndpoints))
		for _, endpoint := range proxyEndpoints {
			reserved[endpoint.Port] = true
		}
		prepared, changed, prepareErr := c.systemProxyManager.EnsureAvoiding(c.platform.SystemProxy.Listen, !running, reserved)
		if prepareErr != nil {
			return false, fmt.Errorf("prepare system proxy: %w", prepareErr)
		}
		systemEndpoint, endpointChanged = &prepared, changed
	}
	candidate, err := config.BuildWithProxyEndpoints(candidateProfile, c.platform, proxyEndpoints, systemEndpoint, now)
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate validation or build failed"})
		return false, err
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
	c.systemProxyEndpoint = systemEndpoint
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
			if before, ok := findNode(oldProfile, n.ID); ok && before.Endpoint != n.Endpoint {
				c.emit(Event{Type: EventNodeEndpointChanged, At: now, Revision: candidateProfile.Revision, NodeID: n.ID})
			}
		}
	}
	if running && endpointChanged && systemEndpoint != nil {
		c.emit(Event{Type: EventSystemProxyEndpointReady, At: now, Revision: candidateProfile.Revision, SystemProxy: endpointsPtr(*systemEndpoint)})
	}
	c.emit(Event{Type: EventProfileApplied, At: now, Revision: candidateProfile.Revision})
	return true, nil
}

func (c *Core) SystemProxyEndpoints() (systemproxy.Endpoints, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	if !c.platform.SystemProxy.Enabled {
		return systemproxy.Endpoints{}, ErrSystemProxyUnavailable
	}
	if c.active == nil || c.systemProxyEndpoint == nil {
		return systemproxy.Endpoints{}, ErrProfileNotApplied
	}
	return systemproxy.Both(*c.systemProxyEndpoint), nil
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
	return localproxy.Credential{}, fmt.Errorf("local proxy node not found")
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
	reverse := map[string]string{}
	if built != nil {
		for id, tag := range built.NodeTags {
			reverse[tag] = id
		}
	}
	for i := range connections {
		connections[i].NodeID = reverse[connections[i].OutboundTag]
	}
	return connections
}

func (c *Core) ProbeEntrances(ctx context.Context, timeout time.Duration, concurrency int) ([]probe.EntranceResult, error) {
	return c.probeEntrances(ctx, timeout, concurrency, nil)
}

func (c *Core) ProbeEntrancesForNodes(
	ctx context.Context,
	timeout time.Duration,
	concurrency int,
	nodeIDs []string,
) ([]probe.EntranceResult, error) {
	return c.probeEntrances(ctx, timeout, concurrency, nodeIDs)
}

func (c *Core) probeEntrances(
	ctx context.Context,
	timeout time.Duration,
	concurrency int,
	nodeIDs []string,
) ([]probe.EntranceResult, error) {
	c.mu.RLock()
	p := c.active
	c.mu.RUnlock()
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
			return nil, fmt.Errorf("one or more probe nodes were not found")
		}
		clone.Nodes = nodes
		clone.Selection.DefaultNodeID = nodes[0].ID
	}
	results, err := probe.Entrances(ctx, clone, timeout, concurrency, nil)
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
func (c *Core) ProbeAvailability(ctx context.Context, nodeID, target string, timeout time.Duration) (probe.AvailabilityResult, error) {
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
	return probe.AvailabilityResult{}, fmt.Errorf("node local proxy endpoint not found")
}

func (c *Core) Start() error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.RLock()
	if c.engine != nil {
		c.mu.RUnlock()
		return nil
	}
	built := c.built
	c.mu.RUnlock()
	if built == nil {
		return fmt.Errorf("no profile applied")
	}
	instance, cancel, err := c.startCandidate(built)
	var migrated *systemproxy.Endpoint
	if err != nil && c.platform.SystemProxy.Enabled && c.systemProxyManager != nil {
		c.mu.RLock()
		reserved := make(map[uint16]bool, len(c.proxyEndpoints))
		for _, localEndpoint := range c.proxyEndpoints {
			reserved[localEndpoint.Port] = true
		}
		c.mu.RUnlock()
		endpoint, changed, reconcileErr := c.systemProxyManager.EnsureAvoiding(c.platform.SystemProxy.Listen, true, reserved)
		if reconcileErr != nil {
			return fmt.Errorf("start runtime: %v; reconcile system proxy: %w", err, reconcileErr)
		}
		if changed {
			c.mu.RLock()
			active := c.active
			localEndpoints := append([]localproxy.Endpoint(nil), c.proxyEndpoints...)
			c.mu.RUnlock()
			rebuilt, buildErr := config.BuildWithProxyEndpoints(active, c.platform, localEndpoints, &endpoint, time.Now())
			if buildErr != nil {
				return fmt.Errorf("rebuild after system proxy migration: %w", buildErr)
			}
			instance, cancel, err = c.startCandidate(rebuilt)
			if err == nil {
				built = rebuilt
				migrated = &endpoint
			}
		}
	}
	if err != nil {
		return fmt.Errorf("start runtime: %w", err)
	}
	c.mu.Lock()
	c.engine, c.cancel, c.built = instance, cancel, built
	if migrated != nil {
		c.systemProxyEndpoint = migrated
	}
	endpoint := c.systemProxyEndpoint
	revision := ""
	if c.active != nil {
		revision = c.active.Revision
	}
	c.mu.Unlock()
	c.emit(Event{Type: EventCoreStarted, At: time.Now()})
	if endpoint != nil {
		c.emit(Event{Type: EventSystemProxyEndpointReady, At: time.Now(), Revision: revision, SystemProxy: endpointsPtr(*endpoint)})
	}
	return nil
}

func (c *Core) Stop() error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.Lock()
	instance, cancel := c.engine, c.cancel
	endpoint := c.systemProxyEndpoint
	c.engine, c.cancel = nil, nil
	c.mu.Unlock()
	if instance == nil {
		return nil
	}
	if cancel != nil {
		cancel()
	}
	err := instance.Close()
	if endpoint != nil {
		c.emit(Event{Type: EventSystemProxyEndpointStopped, At: time.Now(), SystemProxy: endpointsPtr(*endpoint)})
	}
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
		return Status{State: StateStopped, SystemProxy: SystemProxyStatus{Enabled: c.platform.SystemProxy.Enabled}}
	}
	state := StateConfigured
	if c.engine != nil {
		state = StateRunning
	}
	proxyStatus := SystemProxyStatus{Enabled: c.platform.SystemProxy.Enabled, Available: c.engine != nil && c.systemProxyEndpoint != nil}
	if c.systemProxyEndpoint != nil {
		endpoints := systemproxy.Both(*c.systemProxyEndpoint)
		proxyStatus.Endpoints = &endpoints
	}
	return Status{State: state, Revision: c.active.Revision, SelectedNodeID: c.selected, NodeCount: len(c.active.Nodes), SystemProxy: proxyStatus}
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

func hasNode(p *profile.Profile, id string) bool { _, ok := findNode(p, id); return ok }
func findNode(p *profile.Profile, id string) (profile.Node, bool) {
	for _, n := range p.Nodes {
		if n.ID == id {
			return n, true
		}
	}
	return profile.Node{}, false
}

func endpointsPtr(endpoint systemproxy.Endpoint) *systemproxy.Endpoints {
	endpoints := systemproxy.Both(endpoint)
	return &endpoints
}
