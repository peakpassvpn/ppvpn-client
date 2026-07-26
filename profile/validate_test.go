package profile

import (
	"errors"
	"testing"
	"time"
)

func validProfile(protocol Protocol) *Profile {
	p := &Profile{
		SchemaVersion: CurrentSchemaVersion,
		Revision:      "r1",
		ExpiresAt:     time.Now().Add(time.Hour),
		Selection:     Selection{Mode: "manual", DefaultNodeID: "node-1"},
		Routing:       Routing{Final: RoutingAction{Type: "proxy", Target: "selected"}},
	}
	n := Node{ID: "node-1", Name: "Tokyo", Protocol: protocol, Endpoint: Endpoint{Domain: "edge.example.com", IP: "8.8.8.8", Port: 443}, Capabilities: Capabilities{TCP: true, UDP: true}}
	switch protocol {
	case ProtocolShadowsocks:
		n.Credentials.Shadowsocks = &ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA=="}
	case ProtocolVLESS:
		n.Credentials.VLESS = &VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001", Flow: "xtls-rprx-vision"}
		n.TLS = &TLS{ServerName: "edge.example.com", Reality: &Reality{PublicKey: "public", ShortID: "01"}}
	case ProtocolAnyTLS:
		n.Credentials.AnyTLS = &AnyTLSCredentials{Password: "secret"}
		n.TLS = &TLS{ServerName: "edge.example.com"}
	}
	p.Nodes = []Node{n}
	return p
}

func TestSupportedProtocolsValidate(t *testing.T) {
	for _, protocol := range []Protocol{ProtocolShadowsocks, ProtocolVLESS, ProtocolAnyTLS} {
		t.Run(string(protocol), func(t *testing.T) {
			if err := Validate(validProfile(protocol), time.Now()); err != nil {
				t.Fatal(err)
			}
		})
	}
}
func TestReservedEntryIPsRejected(t *testing.T) {
	for _, ip := range []string{"198.18.0.1", "127.0.0.1", "10.0.0.1", "169.254.1.1", "192.0.2.1", "::1", "2001:db8::1"} {
		p := validProfile(ProtocolShadowsocks)
		p.Nodes[0].Endpoint.IP = ip
		err := Validate(p, time.Now())
		var ve *ValidationError
		if !errors.As(err, &ve) || ve.Code != "ENTRY_IP_NOT_PUBLIC" {
			t.Fatalf("%s: %#v", ip, err)
		}
	}
}
func TestSchemaIncompatible(t *testing.T) {
	p := validProfile(ProtocolShadowsocks)
	p.SchemaVersion = CurrentSchemaVersion + 1
	err := Validate(p, time.Now())
	var ve *ValidationError
	if !errors.As(err, &ve) || ve.Code != "SCHEMA_UNSUPPORTED" {
		t.Fatal(err)
	}
}
func TestParseRejectsSingBoxFields(t *testing.T) {
	_, err := Parse([]byte(`{"schema_version":2,"outbounds":[]}`))
	if err == nil {
		t.Fatal("expected unknown field rejection")
	}
}

func TestProtocolFieldsFailClosed(t *testing.T) {
	p := validProfile(ProtocolShadowsocks)
	p.Nodes[0].Credentials.Shadowsocks.ServerKey = "short"
	err := Validate(p, time.Now())
	var ve *ValidationError
	if !errors.As(err, &ve) || ve.Code != "SHADOWSOCKS_KEY_INVALID" {
		t.Fatalf("%#v", err)
	}
	p = validProfile(ProtocolVLESS)
	p.Nodes[0].Credentials.VLESS.UUID = "secret-but-not-uuid"
	err = Validate(p, time.Now())
	if !errors.As(err, &ve) || ve.Field != "nodes[0].credentials.vless" {
		t.Fatalf("%#v", err)
	}
	p = validProfile(ProtocolAnyTLS)
	p.Nodes[0].Transport = &Transport{Type: "ws"}
	err = Validate(p, time.Now())
	if !errors.As(err, &ve) || ve.Code != "TRANSPORT_UNSUPPORTED" {
		t.Fatalf("%#v", err)
	}
}

func TestTLSServerNameMustMatchEndpointDomain(t *testing.T) {
	p := validProfile(ProtocolVLESS)
	p.Nodes[0].TLS.ServerName = "different.example.com"
	err := Validate(p, time.Now())
	var validation *ValidationError
	if !errors.As(err, &validation) || validation.Code != "TLS_SERVER_NAME_MISMATCH" {
		t.Fatalf("%#v", err)
	}
}

func TestRoutingValidationRejectsDuplicateIDsAndUnknownNodes(t *testing.T) {
	p := validProfile(ProtocolShadowsocks)
	p.Routing.Rules = []RoutingRule{
		{ID: "same", Match: RoutingMatch{Protocols: []string{"tcp"}}, Action: RoutingAction{Type: "direct"}},
		{ID: "same", Match: RoutingMatch{Ports: []uint16{443}}, Action: RoutingAction{Type: "reject"}},
	}
	err := Validate(p, time.Now())
	var validation *ValidationError
	if !errors.As(err, &validation) || validation.Code != "RULE_ID_DUPLICATE" {
		t.Fatalf("%#v", err)
	}

	p = validProfile(ProtocolShadowsocks)
	p.Routing.Final = RoutingAction{Type: "proxy", Target: "node", NodeID: "missing"}
	err = Validate(p, time.Now())
	if !errors.As(err, &validation) || validation.Code != "ROUTING_NODE_NOT_FOUND" {
		t.Fatalf("%#v", err)
	}
}

func TestRoutingValidationIDNAPortsCIDRAndStrictJSON(t *testing.T) {
	p := validProfile(ProtocolShadowsocks)
	p.Routing.Rules = []RoutingRule{{
		ID: "complete",
		Match: RoutingMatch{
			Domains:        []string{"例子.测试."},
			DomainSuffixes: []string{"Example.COM"},
			IPCIDRs:        []string{"2001:4860::/32", "1.1.1.0/24"},
			Protocols:      []string{"tcp", "udp"},
			Ports:          []uint16{53, 443},
			PortRanges:     []string{"8000-9000"},
		},
		Action: RoutingAction{Type: "proxy", Target: "selected"},
	}}
	if err := Validate(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if got, ok := NormalizeDomain("例子.测试."); !ok || got != "xn--fsqu00a.xn--0zwm56d" {
		t.Fatalf("IDNA normalization: %q %v", got, ok)
	}
	for _, mutate := range []func(*Profile){
		func(p *Profile) { p.Routing.Rules[0].Match.DomainSuffixes = []string{"*.example.com"} },
		func(p *Profile) { p.Routing.Rules[0].Match.IPCIDRs = []string{"not-cidr"} },
		func(p *Profile) { p.Routing.Rules[0].Match.Protocols = []string{"icmp"} },
		func(p *Profile) { p.Routing.Rules[0].Match.Ports = []uint16{0} },
		func(p *Profile) { p.Routing.Rules[0].Match.PortRanges = []string{"9000-8000"} },
	} {
		bad := validProfile(ProtocolShadowsocks)
		bad.Routing.Rules = append([]RoutingRule(nil), p.Routing.Rules...)
		mutate(bad)
		if err := Validate(bad, time.Now()); err == nil {
			t.Fatal("invalid rule accepted")
		}
	}

	_, err := Parse([]byte(`{
		"schema_version":2,
		"revision":"r",
		"nodes":[],
		"selection":{"mode":"manual","default_node_id":"n"},
		"routing":{"rules":[],"final":{"type":"direct"},"unknown":true}
	}`))
	if err == nil {
		t.Fatal("unknown routing field accepted")
	}
}

func TestRoutingActionUnionIsStrict(t *testing.T) {
	for _, action := range []RoutingAction{
		{Type: "direct", Target: "selected"},
		{Type: "reject", NodeID: "node-1"},
		{Type: "proxy"},
		{Type: "proxy", Target: "selected", NodeID: "node-1"},
		{Type: "proxy", Target: "node"},
	} {
		p := validProfile(ProtocolShadowsocks)
		p.Routing.Final = action
		if err := Validate(p, time.Now()); err == nil {
			t.Fatalf("invalid action accepted: %#v", action)
		}
	}
}
