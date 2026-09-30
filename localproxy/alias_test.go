package localproxy

import (
	"strings"
	"testing"
)

func TestAliasIsShortDeterministicAndOrderIndependent(t *testing.T) {
	const long = "f4d268f0-1869-4913-a3bf-3e9b71a27dbc-773"
	ids := []string{long, "hk-001", "jp-002"}
	first := Aliases(ids)
	again := Aliases([]string{"jp-002", long, "hk-001", long})
	for _, id := range ids {
		alias := first[id]
		if len(alias) != AliasLength || alias != again[id] || alias != aliasDigest(id)[:AliasLength] {
			t.Fatalf("alias %q for %q (again %q)", alias, id, again[id])
		}
		for _, r := range alias {
			if !strings.ContainsRune("abcdefghijklmnopqrstuvwxyz234567", r) {
				t.Fatalf("alias %q has %q", alias, r)
			}
		}
	}
	// A node's alias does not depend on the rest of the profile.
	if alone := Aliases([]string{long}); alone[long] != first[long] {
		t.Fatalf("alias changed with the node set: %q vs %q", alone[long], first[long])
	}
	// Known answers (Python: base64.b32encode(sha256(id)).lower()[:6]).
	if first["hk-001"] != "bwl7qh" || first[long] != "dw4g2d" || Login("jhsup", first[long]) != "jhsup-dw4g2d" {
		t.Fatalf("aliases %#v", first)
	}
}

func TestAliasCollisionsLengthen(t *testing.T) {
	// node-29069/node-33391 share the first 6 alias characters;
	// node-92901/node-792933 share the first 8.
	for _, tc := range []struct {
		a, b   string
		length int
	}{
		{"node-29069", "node-33391", 8},
		{"node-92901", "node-792933", 10},
	} {
		if aliasDigest(tc.a)[:tc.length-2] != aliasDigest(tc.b)[:tc.length-2] {
			t.Fatalf("fixture %s/%s does not collide", tc.a, tc.b)
		}
		for _, order := range [][]string{{tc.a, tc.b, "hk-001"}, {"hk-001", tc.b, tc.a}} {
			aliases := Aliases(order)
			if len(aliases[tc.a]) != tc.length || len(aliases[tc.b]) != tc.length || aliases[tc.a] == aliases[tc.b] {
				t.Fatalf("%v: %#v", order, aliases)
			}
			if aliases[tc.a] != aliasDigest(tc.a)[:tc.length] || len(aliases["hk-001"]) != AliasLength {
				t.Fatalf("%v: %#v", order, aliases)
			}
		}
		// Without the clash each keeps the short alias.
		if alias := Aliases([]string{tc.a})[tc.a]; len(alias) != AliasLength {
			t.Fatalf("lone alias %q", alias)
		}
	}
}

func TestLoginsAndRouteEndpoints(t *testing.T) {
	logins, ok := Logins([]string{FormatUsername("u8f2k", "hk-001"), FormatUsername("u8f2k", "node-29069"), FormatUsername("u8f2k", "node-33391")})
	if !ok || len(logins) != 3 {
		t.Fatalf("logins %#v", logins)
	}
	aliases := Aliases([]string{"hk-001", "node-29069", "node-33391"})
	for _, id := range []string{"hk-001", "node-29069", "node-33391"} {
		if got := logins[FormatUsername("u8f2k", id)]; got != Login("u8f2k", aliases[id]) {
			t.Fatalf("login of %s: %q", id, got)
		}
	}
	if _, ok := Logins([]string{"bad"}); ok {
		t.Fatal("malformed route key accepted")
	}
	if CanonicalLogin("U8F2K-AbC234") != "u8f2k-abc234" {
		t.Fatal("canonical login")
	}
	endpoints := []Endpoint{{NodeID: "hk-001", Username: Login("u8f2k", aliases["hk-001"]), Password: "p"}}
	routed := RouteEndpoints(endpoints)
	if routed[0].Username != "u8f2k-hk-001" || endpoints[0].Username == routed[0].Username || routed[0].Password != "p" {
		t.Fatalf("route endpoints %#v", routed)
	}
}
