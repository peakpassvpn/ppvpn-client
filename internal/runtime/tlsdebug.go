package runtime

import (
	"crypto/sha256"
	"encoding/hex"
	"strings"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

// logIngressTLS writes, at debug level, one line per REALITY or TLS ingress
// of an applied profile with fingerprints of the values the core will use,
// so they can be compared with the server's without logging them: the
// first 10 hex digits of the SHA-256 of the public key and short ID strings
// exactly as received, their lengths and the key's encoding, plus the server
// name, uTLS fingerprint and flow.
func (c *Core) logIngressTLS(p *profile.Profile) {
	if !c.log.DebugEnabled() {
		return
	}
	for _, node := range p.Nodes {
		for _, ingress := range node.Ingresses {
			if ingress.TLS == nil {
				continue
			}
			fields := []any{"node_id", node.ID, "endpoint_key", ingress.EndpointKey, "protocol", ingress.Protocol,
				"server_name", ingress.TLS.ServerName}
			flow := ""
			if ingress.Credentials.VLESS != nil {
				flow = ingress.Credentials.VLESS.Flow
			}
			if reality := ingress.TLS.Reality; reality != nil {
				fields = append(fields,
					"public_key_sha256", shortDigest(reality.PublicKey), "public_key_len", len(reality.PublicKey),
					"public_key_encoding", base64Flavor(reality.PublicKey),
					"short_id_sha256", shortDigest(reality.ShortID), "short_id_len", len(reality.ShortID),
					"fingerprint", "chrome")
			} else {
				fields = append(fields, "insecure", ingress.TLS.Insecure)
			}
			fields = append(fields, "flow", flow)
			c.log.Debug("ingress tls", fields...)
		}
	}
}

func shortDigest(value string) string {
	sum := sha256.Sum256([]byte(value))
	return hex.EncodeToString(sum[:])[:10]
}

// base64Flavor names a base64 string's padding and alphabet from the
// characters it uses: "unpadded url" is base64 raw-url, the form REALITY
// keys use; a key without '-', '_', '+' or '/' fits either alphabet.
func base64Flavor(value string) string {
	padding := "unpadded"
	if strings.HasSuffix(value, "=") {
		padding = "padded"
	}
	alphabet := "url-or-std"
	switch {
	case strings.ContainsAny(value, "+/"):
		alphabet = "std"
	case strings.ContainsAny(value, "-_"):
		alphabet = "url"
	}
	return padding + " " + alphabet
}
