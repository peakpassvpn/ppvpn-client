package localproxy

import (
	"crypto/sha256"
	"encoding/base32"
	"sort"
	"strings"
)

// Proxy usernames have two forms:
//
//   - The login "<prefix>-<alias>" is what clients type and what
//     Endpoint.Username / LocalProxyCredential return. It is short enough to
//     copy into other apps and is matched case-insensitively.
//   - The route key "<prefix>-<nodeID>" (FormatUsername) is internal: the
//     rendered config uses it as the proxy user and the auth_user route rule,
//     so node routing never depends on alias resolution. The inbound maps a
//     login to its route key; the route key itself is not accepted as a login.
//
// The alias is the lowercase RFC 4648 base32 alphabet (a-z, 2-7, no padding)
// encoding of SHA-256(nodeID), truncated to AliasLength characters. It depends
// only on the node ID, so a configured username keeps working across profile
// refreshes while the node ID is stable. When nodes of one profile share an
// alias, every node in the clash is lengthened by aliasStep characters until
// all aliases are distinct; the result depends only on the set of node IDs,
// never on their order.
const (
	AliasLength   = 6
	aliasStep     = 2
	maxAliasChars = (sha256.Size*8 + 4) / 5 // 52: the whole digest
)

var aliasEncoding = base32.StdEncoding.WithPadding(base32.NoPadding)

func aliasDigest(nodeID string) string {
	sum := sha256.Sum256([]byte(nodeID))
	return strings.ToLower(aliasEncoding.EncodeToString(sum[:]))
}

// Aliases returns the unique alias of every node ID in the set.
func Aliases(nodeIDs []string) map[string]string {
	digest := make(map[string]string, len(nodeIDs))
	length := make(map[string]int, len(nodeIDs))
	for _, id := range nodeIDs {
		if _, seen := digest[id]; !seen {
			digest[id] = aliasDigest(id)
			length[id] = AliasLength
		}
	}
	ids := make([]string, 0, len(digest))
	for id := range digest {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	for {
		holders := make(map[string][]string, len(ids))
		for _, id := range ids {
			alias := digest[id][:length[id]]
			holders[alias] = append(holders[alias], id)
		}
		clashed := false
		for _, group := range holders {
			if len(group) < 2 {
				continue
			}
			for _, id := range group {
				// Distinct node IDs cannot share the whole digest in practice;
				// the cap only guarantees termination.
				if length[id] < maxAliasChars {
					length[id] = min(length[id]+aliasStep, maxAliasChars)
					clashed = true
				}
			}
		}
		if !clashed {
			break
		}
	}
	out := make(map[string]string, len(ids))
	for _, id := range ids {
		out[id] = digest[id][:length[id]]
	}
	return out
}

// Login returns the client-facing username "<prefix>-<alias>".
func Login(prefix, alias string) string { return prefix + "-" + alias }

// CanonicalLogin folds a presented username to the form Login returns. The
// prefix and alias alphabets are lowercase, so matching is case-insensitive.
func CanonicalLogin(username string) string { return strings.ToLower(username) }

// Logins maps every route key ("<prefix>-<nodeID>") to its login. Aliases are
// computed over the node IDs of all route keys together. ok is false when a
// route key is malformed.
func Logins(routeKeys []string) (logins map[string]string, ok bool) {
	type parsed struct{ prefix, nodeID string }
	keys := make(map[string]parsed, len(routeKeys))
	ids := make([]string, 0, len(routeKeys))
	for _, key := range routeKeys {
		prefix, nodeID, valid := ParseUsername(key)
		if !valid {
			return nil, false
		}
		keys[key] = parsed{prefix, nodeID}
		ids = append(ids, nodeID)
	}
	aliases := Aliases(ids)
	logins = make(map[string]string, len(keys))
	for key, p := range keys {
		logins[key] = Login(p.prefix, aliases[p.nodeID])
	}
	return logins, true
}

// RouteEndpoints returns copies of endpoints whose Username is the internal
// route key instead of the login, which is what the config builder renders.
func RouteEndpoints(endpoints []Endpoint) []Endpoint {
	out := make([]Endpoint, len(endpoints))
	for i, endpoint := range endpoints {
		out[i] = endpoint
		if prefix, _, ok := ParseUsername(endpoint.Username); ok {
			out[i].Username = FormatUsername(prefix, endpoint.NodeID)
		}
	}
	return out
}
