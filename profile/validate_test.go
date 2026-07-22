package profile

import (
	"errors"
	"testing"
	"time"
)

func validProfile(protocol Protocol) *Profile {
	p := &Profile{SchemaVersion: 1, Revision: "r1", ExpiresAt: time.Now().Add(time.Hour), Selection: Selection{Mode: "manual", DefaultNodeID: "node-1"}}
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
	p.SchemaVersion = 2
	err := Validate(p, time.Now())
	var ve *ValidationError
	if !errors.As(err, &ve) || ve.Code != "SCHEMA_UNSUPPORTED" {
		t.Fatal(err)
	}
}
func TestParseRejectsSingBoxFields(t *testing.T) {
	_, err := Parse([]byte(`{"schema_version":1,"outbounds":[]}`))
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
