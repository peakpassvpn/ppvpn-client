package profile

import "time"

// CurrentSchemaVersion is the only accepted profile format. There is no
// version negotiation: a profile with any other schema_version is rejected.
const CurrentSchemaVersion = 1

type Profile struct {
	SchemaVersion int       `json:"schema_version"`
	Revision      string    `json:"revision"`
	GeneratedAt   time.Time `json:"generated_at"`
	ExpiresAt     time.Time `json:"expires_at"`
	Nodes         []Node    `json:"nodes"`
	Selection     Selection `json:"selection"`
	Routing       Routing   `json:"routing,omitempty"`
}

// Node is a logical node: one exit identity reachable through one or more
// ingresses. Everything that addresses a node (selection, routing node_id,
// local proxies, probes) uses Node.ID; ingresses are an implementation detail
// of how the core reaches that exit.
type Node struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	// EntryKey identifies the entry tier this logical node belongs to (for
	// example "cn-optimized"). It is opaque to the core and passed through to
	// node listings; clients must not assume a fixed set of values.
	EntryKey string `json:"entry_key"`
	// EntryLabel is an optional display name for the entry tier.
	EntryLabel   string       `json:"entry_label,omitempty"`
	Exit         Exit         `json:"exit"`
	Capabilities Capabilities `json:"capabilities"`
	Ingresses    []Ingress    `json:"ingresses"`
}

type IngressRole string

const (
	IngressRolePrimary IngressRole = "primary"
	IngressRoleBackup  IngressRole = "backup"
)

// Ingress is one concrete way to reach a logical node's exit (one replica of
// the node's entry). Array order is failover order: ingresses[0] is the
// primary (role "primary"); every following ingress is a backup (role
// "backup") tried in order while the ones before it are unhealthy.
//
// EndpointKey is the backend's stable identity for the replica (unique within
// the profile); ReplicaOrdinal is its position within the node (unique and
// strictly increasing in array order, not necessarily contiguous).
type Ingress struct {
	Role        IngressRole `json:"role"`
	EndpointKey string      `json:"endpoint_key"`
	// Label is an optional display name (at most 32 characters). It is not
	// an identifier: routing, tags and failover never use it.
	Label          *string      `json:"label,omitempty"`
	ReplicaOrdinal int          `json:"replica_ordinal"`
	Protocol       Protocol     `json:"protocol"`
	Endpoint       Endpoint     `json:"endpoint"`
	Credentials    Credentials  `json:"credentials"`
	TLS            *TLS         `json:"tls,omitempty"`
	Transport      *Transport   `json:"transport,omitempty"`
	Capabilities   Capabilities `json:"capabilities"`
}

// DisplayLabel returns the label, or "" when absent.
func (in Ingress) DisplayLabel() string {
	if in.Label == nil {
		return ""
	}
	return *in.Label
}

// Primary returns the node's primary ingress. It must only be called on a
// validated profile.
func (n Node) Primary() Ingress { return n.Ingresses[0] }

type Protocol string

const (
	ProtocolShadowsocks Protocol = "shadowsocks"
	ProtocolVLESS       Protocol = "vless"
	ProtocolAnyTLS      Protocol = "anytls"
	// Reserved; validation intentionally rejects them.
	ProtocolHysteria2 Protocol = "hysteria2"
	ProtocolTrojan    Protocol = "trojan"
	ProtocolWireGuard Protocol = "wireguard"
)

// Endpoint is an ingress address. Domain is always required (it is also the
// TLS server name); IP is optional and, when present, must be a public
// unicast address that is used for probing and TUN route exclusion.
type Endpoint struct {
	Domain string `json:"domain"`
	IP     string `json:"ip,omitempty"`
	Port   uint16 `json:"port"`
}
type Exit struct {
	IP     string `json:"ip,omitempty"`
	Region string `json:"region,omitempty"`
	// CountryCode is the exit's ISO 3166-1 alpha-2 code (display only).
	CountryCode string `json:"country_code,omitempty"`
}
type Capabilities struct {
	TCP bool `json:"tcp"`
	UDP bool `json:"udp"`
}
type Selection struct {
	Mode          string `json:"mode"`
	DefaultNodeID string `json:"default_node_id"`
}

type Routing struct {
	Rules []RoutingRule `json:"rules,omitempty"`
	Final RoutingAction `json:"final"`
}

// RoutingRule is evaluated in array order. Alternatives within the address
// group and within the port group are ORed; the non-empty address, protocol,
// and port groups are ANDed.
type RoutingRule struct {
	ID     string        `json:"id"`
	Match  RoutingMatch  `json:"match"`
	Action RoutingAction `json:"action"`
}

type RoutingMatch struct {
	Domains        []string `json:"domains,omitempty"`
	DomainSuffixes []string `json:"domain_suffixes,omitempty"`
	IPCIDRs        []string `json:"ip_cidrs,omitempty"`
	IPIsPrivate    bool     `json:"ip_is_private,omitempty"`
	Protocols      []string `json:"protocols,omitempty"`
	Ports          []uint16 `json:"ports,omitempty"`
	PortRanges     []string `json:"port_ranges,omitempty"`
}

type RoutingAction struct {
	Type   string `json:"type"`
	Target string `json:"target,omitempty"`
	NodeID string `json:"node_id,omitempty"`
}

// Credentials is a tagged union. Exactly one member matching Ingress.Protocol is required.
type Credentials struct {
	Shadowsocks *ShadowsocksCredentials `json:"shadowsocks,omitempty"`
	VLESS       *VLESSCredentials       `json:"vless,omitempty"`
	AnyTLS      *AnyTLSCredentials      `json:"anytls,omitempty"`
}

// ShadowsocksCredentials follow SIP022 naming. IdentityKeys are the server's
// identity PSKs (iPSKs) for the Extensible Identity Headers, ordered outermost
// first; they may be absent for single-user SS2022. UserKey is this user's
// PSK (uPSK). The core builds the password iPSK1:...:iPSKn:uPSK.
type ShadowsocksCredentials struct {
	Method       string   `json:"method"`
	IdentityKeys []string `json:"identity_keys,omitempty"`
	UserKey      string   `json:"user_key"`
}
type VLESSCredentials struct {
	UUID string `json:"uuid"`
	Flow string `json:"flow,omitempty"`
}
type AnyTLSCredentials struct {
	Password string `json:"password"`
}
type TLS struct {
	ServerName string   `json:"server_name"`
	ALPN       []string `json:"alpn,omitempty"`
	Insecure   bool     `json:"insecure,omitempty"`
	Reality    *Reality `json:"reality,omitempty"`
}
type Reality struct {
	PublicKey string `json:"public_key"`
	ShortID   string `json:"short_id"`
}
type Transport struct {
	Type string `json:"type,omitempty"`
}

type PlatformCapabilities struct {
	Platform   string                 `json:"platform"`
	TUN        TUNCapabilities        `json:"tun"`
	LocalProxy LocalProxyCapabilities `json:"local_proxy"`
	LogLevel   string                 `json:"log_level"`
}
type TUNCapabilities struct {
	Enabled bool   `json:"enabled"`
	Stack   string `json:"stack,omitempty"`
}
type LocalProxyCapabilities struct {
	Enabled bool   `json:"enabled"`
	Listen  string `json:"listen,omitempty"`
}
