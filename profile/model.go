package profile

import "time"

const CurrentSchemaVersion = 2

type Profile struct {
	SchemaVersion int       `json:"schema_version"`
	Revision      string    `json:"revision"`
	GeneratedAt   time.Time `json:"generated_at"`
	ExpiresAt     time.Time `json:"expires_at"`
	Nodes         []Node    `json:"nodes"`
	Selection     Selection `json:"selection"`
	Routing       Routing   `json:"routing,omitempty"`
}

type Node struct {
	ID           string       `json:"id"`
	Name         string       `json:"name"`
	Protocol     Protocol     `json:"protocol"`
	Endpoint     Endpoint     `json:"endpoint"`
	Exit         *Exit        `json:"exit,omitempty"`
	Credentials  Credentials  `json:"credentials"`
	TLS          *TLS         `json:"tls,omitempty"`
	Transport    *Transport   `json:"transport,omitempty"`
	Capabilities Capabilities `json:"capabilities"`
}

type Protocol string

const (
	ProtocolShadowsocks Protocol = "shadowsocks"
	ProtocolVLESS       Protocol = "vless"
	ProtocolAnyTLS      Protocol = "anytls"
	// Reserved for later schema revisions; v2 validation intentionally rejects them.
	ProtocolHysteria2 Protocol = "hysteria2"
	ProtocolTrojan    Protocol = "trojan"
	ProtocolWireGuard Protocol = "wireguard"
)

type Endpoint struct {
	Domain string `json:"domain"`
	IP     string `json:"ip"`
	Port   uint16 `json:"port"`
}
type Exit struct {
	IP          string `json:"ip,omitempty"`
	Region      string `json:"region,omitempty"`
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

// Credentials is a tagged union. Exactly one member matching Node.Protocol is required.
type Credentials struct {
	Shadowsocks *ShadowsocksCredentials `json:"shadowsocks,omitempty"`
	VLESS       *VLESSCredentials       `json:"vless,omitempty"`
	AnyTLS      *AnyTLSCredentials      `json:"anytls,omitempty"`
}
type ShadowsocksCredentials struct {
	Method       string   `json:"method"`
	ServerKey    string   `json:"server_key"`
	IdentityKeys []string `json:"identity_keys,omitempty"`
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
