package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"strings"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/dnstransport"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/internal/hostipv6"
	"github.com/peakpassvpn/ppvpn-core/internal/outboundlog"
	"github.com/peakpassvpn/ppvpn-core/internal/proxyinbound"
	"github.com/peakpassvpn/ppvpn-core/internal/reversemap"
	"github.com/peakpassvpn/ppvpn-core/internal/rulesets"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/probe"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/routing"
	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
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
	// RuleSets reports every rule set of the applied profile, in profile
	// order; omitted when the profile declares none.
	RuleSets []rulesets.Status `json:"rule_sets,omitempty"`
	// RoutingMode is the mode the applied profile runs in; omitted before
	// a profile is applied.
	RoutingMode RoutingMode `json:"routing_mode,omitempty"`
	// Nodes reports each node's ingress pin and health, in profile order.
	Nodes []NodeStatus `json:"nodes,omitempty"`
	// DrainingKernels counts kernels replaced by an apply that still serve
	// their connections (0.5.17).
	DrainingKernels int `json:"draining_kernels"`
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
	// ruleSets owns the profile's rule set files; allowedRuleSetHosts are
	// the hosts the applied profile's rule set URLs were pinned to.
	ruleSets            *rulesets.Manager
	allowedRuleSetHosts []string
	// hostIPv6 probes whether the desktop TUN can carry IPv6 on this host;
	// tests replace it.
	hostIPv6 func() bool
	// hostIPv6Route probes whether the host has an IPv6 path of its own
	// (hostipv6.Route); tests replace it.
	hostIPv6Route func() (bool, error)
	// routingMode is the mode the active profile was applied in.
	routingMode RoutingMode
	// pins maps a node ID to the endpoint_key it is pinned to (PinIngress).
	pins map[string]string
	// log is the first-party diagnostic log (phase timings; per-connection
	// lines at debug level).
	log *corelog.Logger
	// reverse is the address → domain mapping of DNS answers, shared by every
	// kernel so a kernel switch keeps it.
	reverse *reversemap.Store
}

// SetLogger sets the diagnostic log. It must be called before the first
// ApplyProfile.
func (c *Core) SetLogger(log *corelog.Logger) {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.log = log
}

// ApplyOptions are the host-supplied inputs of apply-profile besides the
// profile itself.
type ApplyOptions struct {
	// AllowedRuleSetHosts are the authorities (host or host:port) of the API
	// the profile was fetched from. Every rule set URL must be on one of
	// them; without any, rule sets are never downloaded.
	AllowedRuleSetHosts []string
	// RoutingMode selects which profile rules apply; empty means rules. A
	// different mode re-applies a profile even when its revision is
	// unchanged.
	RoutingMode RoutingMode
}

// RuleSetPrepareTimeout bounds how long apply-profile waits for rule set
// downloads before building with whatever is available.
const RuleSetPrepareTimeout = 10 * time.Second

func New(platform profile.PlatformCapabilities) *Core {
	return newCore(platform, newSingBox)
}

func NewWithLocalProxyState(platform profile.PlatformCapabilities, statePath string) *Core {
	core := newCore(platform, newSingBox)
	core.proxyManager = localproxy.NewManager(statePath)
	return core
}

// hostIPv6Route is the host IPv6 path probe new cores use (the runtime
// tests pin it, so they do not depend on the machine's network).
var hostIPv6Route = hostipv6.Route

func newCore(platform profile.PlatformCapabilities, factory engineFactory) *Core {
	key, keyOK := newFlowAuthorizationKey()
	core := &Core{
		platform:             platform,
		bus:                  newEventBus(),
		factory:              factory,
		flowAuthorizationKey: key,
		flowAuthorizationOK:  keyOK,
		hostIPv6:             hostipv6.Available,
		hostIPv6Route:        hostIPv6Route,
		log:                  corelog.Discard(),
		reverse:              reversemap.New(),
	}
	core.ruleSets = rulesets.New(core.ruleSetOptions(""))
	return core
}

// EnableRuleSets stores rule set files in dir (normally
// <state_dir>/rule-sets). Without it, rule sets are always unavailable. It
// must be called before the first ApplyProfile.
func (c *Core) EnableRuleSets(dir string) { c.enableRuleSets(dir, nil) }

// enableRuleSets lets tests adjust the manager options.
func (c *Core) enableRuleSets(dir string, adjust func(*rulesets.Options)) {
	c.operation.Lock()
	defer c.operation.Unlock()
	options := c.ruleSetOptions(dir)
	if adjust != nil {
		adjust(&options)
	}
	c.mu.Lock()
	previous := c.ruleSets
	c.ruleSets = rulesets.New(options)
	c.mu.Unlock()
	previous.Close()
}

func (c *Core) ruleSetOptions(dir string) rulesets.Options {
	return rulesets.Options{
		Dir:  dir,
		Dial: c.dialRuleSet,
		OnState: func(status rulesets.Status) {
			c.logRuleSet(status)
			c.emit(Event{Type: EventRuleSetChanged, At: time.Now(), RuleSetID: status.ID, Message: string(status.State), Code: status.Error})
		},
		OnRebuild: c.rebuildForRuleSets,
	}
}

// dialRuleSet opens the direct connection of a rule set download. While a
// TUN instance runs, it goes through that instance's direct outbound, which
// is bound to the physical interface (auto_detect_interface), so it can
// neither enter the tunnel nor reach a node. Otherwise no tunnel of this
// core exists and a plain socket is direct.
func (c *Core) dialRuleSet(ctx context.Context, network, address string) (net.Conn, error) {
	c.mu.RLock()
	instance := c.engine
	c.mu.RUnlock()
	if instance != nil && c.platform.TUN.Enabled {
		direct, ok := instance.(directEngine)
		if !ok {
			return nil, fmt.Errorf("runtime has no direct dialer")
		}
		return direct.dialDirect(ctx, network, address)
	}
	return (&net.Dialer{Timeout: 10 * time.Second}).DialContext(ctx, network, address)
}

// rebuildForRuleSets rebuilds the configuration after a background refresh
// changed which rule sets are available.
func (c *Core) rebuildForRuleSets() {
	if err := c.reload(); err != nil && !errors.Is(err, ErrProfileNotApplied) {
		c.emit(Event{Type: EventReloadFailed, At: time.Now(), Message: "rule set rebuild failed"})
	}
}

// logRuleSet writes one info line per rule set state change.
func (c *Core) logRuleSet(status rulesets.Status) {
	fields := []any{"id", status.ID, "state", status.State}
	if status.Error != "" {
		fields = append(fields, "error", status.Error, "failures", status.Failures)
	}
	if status.NextRetryAt != nil {
		fields = append(fields, "next_retry_at", status.NextRetryAt.Format(time.RFC3339))
	}
	c.log.Info("rule set", fields...)
}

func (c *Core) ApplyProfile(p *profile.Profile, now time.Time) (bool, error) {
	return c.ApplyProfileWithOptions(p, now, ApplyOptions{})
}

func (c *Core) ApplyProfileWithOptions(p *profile.Profile, now time.Time, options ApplyOptions) (bool, error) {
	c.operation.Lock()
	defer c.operation.Unlock()
	mode, err := ParseRoutingMode(string(options.RoutingMode))
	if err != nil {
		return false, err
	}
	return c.applyProfileLocked(p, now, append([]string(nil), options.AllowedRuleSetHosts...), mode, false)
}

// applyProfileLocked applies p in mode. allowedHosts pins its rule set URLs.
// The same revision in the same mode is a no-op, except for a rebuild
// (reload, rule set refresh), which re-applies the active profile as is and
// only uses cached rule sets (the refresh loop owns retries).
func (c *Core) applyProfileLocked(p *profile.Profile, now time.Time, allowedHosts []string, mode RoutingMode, rebuild bool) (applied bool, err error) {
	downloadRuleSets := !rebuild
	timer := newPhaseTimer()
	var setsReady, setsStale, setsUnavailable int
	defer func() {
		if applied || err != nil {
			timer.log(c.log, "apply timing", err, "tun", c.platform.TUN.Enabled, "rebuild", rebuild,
				"rule_sets_ready", setsReady, "rule_sets_stale", setsStale, "rule_sets_unavailable", setsUnavailable)
		}
	}()
	c.mu.RLock()
	if !rebuild && c.active != nil && p != nil && c.active.Revision == p.Revision && c.routingMode == mode {
		c.mu.RUnlock()
		return false, nil
	}
	oldProfile, oldBuilt, oldInstance, oldInstanceCancel := c.active, c.built, c.engine, c.cancel
	running := oldInstance != nil
	selected := c.selected
	c.mu.RUnlock()

	var candidateProfile *profile.Profile
	candidateProfile, err = cloneProfile(p)
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate profile copy failed"})
		return false, err
	}
	if selected != "" && hasNode(candidateProfile, selected) {
		candidateProfile.Selection.DefaultNodeID = selected
	}
	// Validate before any rule set URL is contacted.
	if err = profile.Validate(candidateProfile, now); err == nil && len(allowedHosts) > 0 {
		err = profile.ValidateRuleSetHosts(candidateProfile, allowedHosts)
	}
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate validation or build failed"})
		return false, stageError("apply/build", err)
	}
	// Rule sets, the sing-box options and the flow classifier are all built
	// from the rules the mode keeps; c.active keeps the whole profile.
	effective, err := effectiveProfile(candidateProfile, mode)
	if err != nil {
		return false, stageError("apply/build", err)
	}
	timer.mark("validate")
	var ruleSets *rulesets.Snapshot
	{
		ctx, cancel := context.WithTimeout(context.Background(), RuleSetPrepareTimeout)
		ruleSets = c.ruleSets.Prepare(ctx, effective.Routing.RuleSets, allowedHosts, downloadRuleSets)
		cancel()
	}
	timer.mark("rule_sets")
	setsReady, setsStale, setsUnavailable = ruleSets.Counts()
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
			return false, stageError("apply/local-proxy-state", fmt.Errorf("prepare local proxies: %w", err))
		}
	}
	timer.mark("local_proxy")
	disableIPv6, noIPv6Route := c.hostIPv6State()
	timer.mark("host_ipv6")
	candidate, err := config.BuildWithOptions(effective, c.platform, config.BuildOptions{LocalProxies: proxyEndpoints, RuleSets: ruleSets.Files(), DisableTUNIPv6: disableIPv6, NoHostIPv6Route: noIPv6Route}, now)
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate validation or build failed"})
		return false, stageError("apply/build", err)
	}
	systemPort := uint16(0)
	if c.systemProxyEnabled {
		// A running listener keeps its port; before start, re-check it.
		if systemPort, err = c.proxyManager.SystemProxyPort(!running, sharedPort(proxyEndpoints)); err != nil {
			return false, stageError("apply/system-proxy", fmt.Errorf("prepare system proxy: %w", err))
		}
		candidate = config.WithSystemProxy(candidate, systemPort)
	}
	timer.mark("build")
	candidateClassifier, err := routing.Compile(effective, now)
	if err != nil {
		c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate routing compilation failed"})
		return false, stageError("apply/routing", err)
	}
	timer.mark("routing")

	var replacement engine
	var replacementCancel context.CancelFunc
	// A running layered engine takes the candidate as a new kernel without
	// closing listeners or connections; only a listener change (the
	// fullRestartReasons whitelist) needs the stop-and-start path below.
	var switched *kernelEvent
	if running {
		if swapper, ok := oldInstance.(swapEngine); ok {
			if reasons := swapper.fullRestartReasons(candidate.Options); len(reasons) == 0 {
				event, err := swapper.swap(c.kernelContext(candidate), candidate.Options,
					func(next engine) { c.applyPins(next, candidate) },
					closeOnSwitch(oldBuilt, candidateProfile, candidate))
				timer.mark("kernel_switch")
				if err != nil {
					c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate kernel start failed"})
					return false, stageError("apply", fmt.Errorf("start candidate kernel: %w", err))
				}
				switched = &event
				replacement, replacementCancel = oldInstance, oldInstanceCancel
			} else {
				c.log.Info("apply full restart", "reasons", strings.Join(reasons, "; "))
			}
		}
	}
	reusePorts := running && switched == nil && len(candidate.Options.Inbounds) > 0
	if running && switched == nil {
		if reusePorts {
			if oldInstanceCancel != nil {
				oldInstanceCancel()
			}
			_ = oldInstance.Close()
		}
		replacement, replacementCancel, err = c.startCandidate(candidate, timer)
		if err != nil {
			if reusePorts && oldBuilt != nil {
				rollback, rollbackCancel, rollbackErr := c.startCandidate(oldBuilt, nil)
				c.mu.Lock()
				c.engine, c.cancel = rollback, rollbackCancel
				c.mu.Unlock()
				if rollbackErr != nil {
					c.mu.Lock()
					c.engine, c.cancel = nil, nil
					c.mu.Unlock()
					c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate start and runtime rollback failed"})
					return false, stageError("apply/rollback", fmt.Errorf("start candidate: %v; rollback runtime: %w", err, rollbackErr))
				}
			}
			c.emit(Event{Type: EventReloadFailed, At: now, Message: "candidate runtime start failed"})
			return false, stageError("apply", fmt.Errorf("start candidate runtime: %w", err))
		}
	}

	c.mu.Lock()
	oldEngine, oldCancel := c.engine, c.cancel
	c.active, c.built, c.classifier = candidateProfile, candidate, candidateClassifier
	c.routingMode = mode
	pinEvents := c.prunePinsLocked(candidateProfile, now)
	c.routingGeneration++
	c.proxyEndpoints = proxyEndpoints
	c.allowedRuleSetHosts = allowedHosts
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

	c.ruleSets.Activate(ruleSets)
	if switched != nil {
		c.kernelSwitched(*switched, candidateProfile.Revision, now)
	}
	if oldCancel != nil && running && !reusePorts && switched == nil {
		oldCancel()
	}
	if oldEngine != nil && running && !reusePorts && switched == nil {
		_ = oldEngine.Close()
	}
	if oldProfile != nil {
		for _, n := range candidateProfile.Nodes {
			if before, ok := findNode(oldProfile, n.ID); ok && !sameIngressEndpoints(before, n) {
				c.emit(Event{Type: EventNodeEndpointChanged, At: now, Revision: candidateProfile.Revision, NodeID: n.ID})
			}
		}
	}
	if !rebuild {
		c.logIngressTLS(candidateProfile)
	}
	for _, event := range pinEvents {
		c.emit(event)
	}
	c.emit(Event{Type: EventProfileApplied, At: now, Revision: candidateProfile.Revision})
	return true, nil
}

func (c *Core) LocalProxyEndpoints() []localproxy.Endpoint {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return append([]localproxy.Endpoint(nil), c.proxyEndpoints...)
}

// LocalProxyMetadata lists one entry per node (kind "node", by node id) and,
// last, the routed user (kind "routed", empty node id); last so hosts that
// read the first entry keep getting a node.
func (c *Core) LocalProxyMetadata() []localproxy.Metadata {
	c.mu.RLock()
	defer c.mu.RUnlock()
	result := make([]localproxy.Metadata, 0, len(c.proxyEndpoints)+1)
	entry := func(kind string, endpoint localproxy.Endpoint) localproxy.Metadata {
		return localproxy.Metadata{
			Kind:         kind,
			NodeID:       endpoint.NodeID,
			Listen:       endpoint.Listen,
			Port:         endpoint.Port,
			Protocols:    []string{"http", "socks5"},
			AuthRequired: true,
		}
	}
	for _, endpoint := range c.proxyEndpoints {
		result = append(result, entry(localproxy.KindNode, endpoint))
	}
	if routed, ok := localproxy.RoutedEndpoint(c.proxyEndpoints); ok {
		result = append(result, entry(localproxy.KindRouted, routed))
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
				Kind:     localproxy.KindNode,
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

// LocalProxyRoutedCredential returns the routed user's credential: traffic
// sent with it is routed like the system proxy's, by the profile rules and
// then the selected node.
func (c *Core) LocalProxyRoutedCredential() (localproxy.Credential, error) {
	if !c.platform.LocalProxy.Enabled {
		return localproxy.Credential{}, ErrLocalProxyDisabled
	}
	c.mu.RLock()
	defer c.mu.RUnlock()
	routed, ok := localproxy.RoutedEndpoint(c.proxyEndpoints)
	if !ok {
		return localproxy.Credential{}, ErrProfileNotApplied
	}
	return localproxy.Credential{Kind: localproxy.KindRouted, Listen: routed.Listen, Port: routed.Port, Username: routed.Username, Password: routed.Password}, nil
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

func (c *Core) Start() (err error) {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.RLock()
	if c.engine != nil {
		c.mu.RUnlock()
		return nil
	}
	timer := newPhaseTimer()
	defer func() { timer.log(c.log, "start timing", err, "tun", c.platform.TUN.Enabled) }()
	built, selected, systemPort := c.built, c.selected, c.systemProxyPort
	localPort := sharedPort(c.proxyEndpoints)
	c.mu.RUnlock()
	if built == nil {
		return fmt.Errorf("no profile applied")
	}
	if c.platform.TUN.Enabled {
		// The host may have joined or left an IPv6 network since the last
		// apply: rebuild when its IPv6 path changed.
		if _, noIPv6Route := c.hostIPv6State(); noIPv6Route != built.DirectIPv6HandOff {
			c.mu.RLock()
			active, hosts, mode := c.active, c.allowedRuleSetHosts, c.routingMode
			c.mu.RUnlock()
			if _, err := c.applyProfileLocked(active, time.Now(), hosts, mode, true); err != nil {
				return stageError("start/host-ipv6", err)
			}
			c.mu.RLock()
			built = c.built
			c.mu.RUnlock()
		}
	}
	if c.systemProxyEnabled {
		// The port may have been taken while the core was stopped.
		port, err := c.proxyManager.SystemProxyPort(true, localPort)
		if err != nil {
			return stageError("start/system-proxy", fmt.Errorf("prepare system proxy: %w", err))
		}
		if port != systemPort {
			built, systemPort = config.WithSystemProxy(built, port), port
		}
		timer.mark("system_proxy")
	}
	instance, cancel, err := c.startCandidate(built, timer)
	if err != nil {
		return stageError("start", fmt.Errorf("start runtime: %w", err))
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
	err := c.reload()
	if errors.Is(err, ErrProfileNotApplied) {
		return fmt.Errorf("no profile applied")
	}
	return err
}

func (c *Core) reload() error {
	c.operation.Lock()
	defer c.operation.Unlock()
	c.mu.RLock()
	p := c.active
	if p == nil {
		c.mu.RUnlock()
		return ErrProfileNotApplied
	}
	clone, err := cloneProfile(p)
	allowedHosts, mode := c.allowedRuleSetHosts, c.routingMode
	c.mu.RUnlock()
	if err != nil {
		return err
	}
	_, err = c.applyProfileLocked(clone, time.Now(), allowedHosts, mode, true)
	return err
}

// outboundIngresses names the node and ingress of every node outbound tag
// (for the outbound failure log). A single-ingress node's outbound is the
// node tag itself.
func outboundIngresses(built *config.BuildResult) map[string]outboundlog.Ingress {
	out := make(map[string]outboundlog.Ingress, len(built.OutboundNodes))
	for tag, node := range built.OutboundNodes {
		if key, ok := built.IngressKeys[tag]; ok {
			out[tag] = outboundlog.Ingress{NodeID: node, EndpointKey: key}
		}
	}
	return out
}

// startCandidate creates and starts an engine; timer (optional) records
// engine_create (sing-box option parsing and object setup) and engine_start
// (sing-box start: outbounds, DNS, router and rule sets, inbounds including
// opening the TUN and installing its routes).
func (c *Core) startCandidate(candidate *config.BuildResult, timer *phaseTimer) (engine, context.CancelFunc, error) {
	ctx, cancel := context.WithCancel(c.kernelContext(candidate))
	instance, err := c.factory(ctx, candidate.Options)
	if err != nil {
		cancel()
		return nil, nil, stageError("engine-create", err)
	}
	if logged, ok := instance.(connectionLogEngine); ok {
		logged.setConnectionLog(c.log)
	}
	if swapper, ok := instance.(swapEngine); ok {
		swapper.setKernelEvents(c.kernelDrained)
	}
	c.applyPins(instance, candidate)
	timer.mark("engine_create")
	err = instance.Start()
	timer.mark("engine_start")
	if err == nil {
		if watcher, ok := instance.(interfaceWatchEngine); ok {
			watcher.watchDefaultInterface(c.log)
		}
	}
	if err != nil {
		cancel()
		_ = instance.Close()
		stage := "engine-start"
		if c.platform.TUN.Enabled && strings.Contains(err.Error(), "inbound/tun[") {
			stage = "engine-start/tun-open"
		}
		return nil, nil, stageError(stage, err)
	}
	return instance, cancel, nil
}

// hostIPv6State probes the host's IPv6 for a TUN build: disable leaves IPv6
// out of the TUN (stack disabled, see hostipv6.Available); noRoute hands
// direct IPv6 destinations their domain (stack enabled but no IPv6 path, see
// config.BuildOptions.NoHostIPv6Route). Without a TUN both are false. The
// result is logged on every probe; a failed route probe keeps IPv6 as before
// and logs why.
func (c *Core) hostIPv6State() (disable, noRoute bool) {
	if !c.platform.TUN.Enabled {
		return false, false
	}
	enabled := c.hostIPv6()
	route := false
	if enabled {
		var err error
		if route, err = c.hostIPv6Route(); err != nil {
			c.log.Warn("host ipv6 route probe failed", "error", err, "assumed_route", route)
		}
	}
	policy := "tun_ipv6"
	switch {
	case !enabled:
		policy = "tun_ipv4_only"
	case !route:
		policy = "tun_ipv6_direct_ipv4"
	}
	c.log.Info("host ipv6", "host_ipv6_enabled", enabled, "host_ipv6_route", route, "policy", policy)
	return !enabled, enabled && !route
}

func drainingKernels(instance engine) int {
	if swapper, ok := instance.(swapEngine); ok {
		return swapper.drainingKernels()
	}
	return 0
}

// kernelContext carries what the engine built from candidate needs: the
// ingress switch observer and the diagnostic loggers. It carries no service
// registry, so every box made from it gets its own.
func (c *Core) kernelContext(candidate *config.BuildResult) context.Context {
	ctx := context.Background()
	ctx = failover.WithSwitchObserver(ctx, c.ingressObserver(candidate))
	ctx = dnstransport.WithLogger(ctx, c.log)
	if c.platform.TUN.Enabled {
		// Only the TUN configuration turns dns.reverse_mapping on.
		ctx = reversemap.WithStore(ctx, c.reverse)
	}
	return outboundlog.WithLogger(ctx, c.log, outboundIngresses(candidate))
}

// closeOnSwitch decides which connections of the replaced kernel to close
// when next takes over: only those the new profile takes away. A connection
// is closed when the node it uses is gone, when it came in as a local proxy
// user the new list no longer has, or when the new kernel's route rules
// would now reject it. Everything else (another node or outbound, direct
// instead of proxy, a global/rules switch) keeps running until it ends.
func closeOnSwitch(old *config.BuildResult, next *profile.Profile, candidate *config.BuildResult) func(engine, trackedView) bool {
	nodes := make(map[string]bool, len(next.Nodes))
	for _, node := range next.Nodes {
		nodes[node.ID] = true
	}
	users := map[string]bool{}
	for _, inbound := range candidate.Options.Inbounds {
		if options, ok := inbound.Options.(*proxyinbound.Options); ok {
			for _, user := range options.Users {
				users[user.Username] = true
			}
		}
	}
	return func(nextEngine engine, item trackedView) bool {
		if old != nil {
			for _, tag := range append([]string{item.outboundTag}, item.route...) {
				if node, ok := old.OutboundNodes[tag]; ok && !nodes[node] {
					return true
				}
			}
		}
		if item.metadata.Inbound == config.LocalProxyInboundTag && item.metadata.User != "" && !users[item.metadata.User] {
			return true
		}
		if kernel, ok := nextEngine.(*singEngine); ok {
			return rejectedBy(kernel.Router().Rules(), item.metadata)
		}
		return false
	}
}

// rejectedBy reports whether the first final route rule matching metadata
// rejects it. Rules that only set options (sniff, resolve, route-options)
// are passed over, as the router does.
func rejectedBy(rules []adapter.Rule, metadata adapter.InboundContext) bool {
	for _, rule := range rules {
		probe := metadata
		if !rule.Match(&probe) {
			continue
		}
		switch rule.Action().Type() {
		case C.RuleActionTypeReject:
			return true
		case C.RuleActionTypeSniff, C.RuleActionTypeResolve, C.RuleActionTypeRouteOptions:
			continue
		default:
			return false
		}
	}
	return false
}

// kernelSwitched logs and reports a kernel switch made by an apply.
func (c *Core) kernelSwitched(event kernelEvent, revision string, now time.Time) {
	c.log.Info("kernel switched", "gen", event.Gen, "previous", event.Previous, "closed_connections", event.Closed, "kept_connections", event.Kept)
	c.emit(Event{Type: EventKernelSwitched, At: now, Revision: revision, ClosedConnections: event.Closed, KeptConnections: event.Kept})
}

// kernelDrained logs and reports a replaced kernel that was closed.
func (c *Core) kernelDrained(event kernelEvent) {
	c.log.Info("kernel drained", "gen", event.Gen, "reason", event.Reason, "closed_connections", event.Closed)
	c.emit(Event{Type: EventKernelDrained, At: time.Now(), Code: event.Reason, ClosedConnections: event.Closed})
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
	return Status{State: state, Revision: c.active.Revision, SelectedNodeID: c.selected, NodeCount: len(c.active.Nodes), SelectedIngress: c.selectedIngressLocked(), SystemProxy: c.systemProxyStatusLocked(), RuleSets: c.ruleSets.Statuses(), RoutingMode: c.routingMode, Nodes: c.nodeStatusesLocked(), DrainingKernels: drainingKernels(c.engine)}
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
