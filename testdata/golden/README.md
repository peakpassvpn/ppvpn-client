# Golden files

Behaviour baselines exported from the Go core (frozen at v0.5.21), for the
Rust `ppvpn-engine` to be checked against (#45). Every file is
language-independent JSON; Go checks itself against the same files, so the
baseline cannot drift.

| Path | What | Go runner |
| --- | --- | --- |
| `options.json`, `options-multi-ingress.json` | sing-box options built from `testdata/profiles/*` | `internal/config/golden_test.go` (sing-box specific; not a Rust baseline) |
| `contract/` | Core API v1 contract: request sequences and the responses and events they produce | `api/golden_contract_test.go` |
| `routing/` | routing decisions of the real engine for a profile and a set of connections | `internal/runtime/golden_routing_test.go` |

Regenerate after an intended behaviour change, then review the diff:

```sh
go test ./api -run TestGoldenContract -update
go test ./internal/runtime -run TestGoldenRouting -update
```

Running `-update` twice gives byte-identical files; anything random (ports,
times, map order) is left out or normalised.

## contract/

One file per scenario:

```json
{
  "name": "pin_ingress",
  "description": "…",
  "platform": { … PlatformCapabilities, optional … },
  "steps": [
    { "name": "pin_backup", "method": "pin-ingress",
      "body": { "node_id": "…", "endpoint_key": "9002" },
      "profile_ref": { "base": "profiles/base.json", "patch": [ … ] } }
  ],
  "expect": [
    { "status": 200,
      "response": { "ok": true, "data": { … } },
      "events": [ { "type": "NodeIngressPinned", … } ] }
  ]
}
```

- **Steps.** Each step is one Core API v1 call: `POST /v1/<method>` with `body`, against one core created with `platform`. Steps run in order on the same core.
- **Profiles.** `profile_ref`, when present, becomes the body's `profile`: the file `base` (relative to `contract/`) with the RFC 6902 JSON Patch `patch` applied (`add`, `remove` and `replace` are used).
- **Expectations.** `expect[i]` belongs to `steps[i]`:
  - `status` is the HTTP status;
  - `response` is the envelope;
  - `events` are the events emitted while the step ran, in order.
- **What is left out.** These are not contract or not deterministic:
  - `request_id`;
  - `error.message`, which is free text. The contract is `error.code`, `error.field` and `error.retryable`;
  - every event's `at`;
  - any other timestamp. A key named `*_at` holds `"<time>"` when set.
- **Comparison.** JSON values are compared, not text: key order and whitespace do not matter.
- **Rust engine.** It drives the same steps through its library API and maps the result to the same envelope and events. A method with no library equivalent must be listed in `docs/rust-parity.md` with the reason.
- **Unexpected outcomes are recorded as they are.** Some outcomes are surprising and are kept on purpose, because they are what hosts see today:
  - `start` before any profile, and `select-node` with an unknown node, fold to `CORE_OPERATION_FAILED`;
  - an apply whose `default_node_id` no longer exists is accepted, and the current selection is kept.

  Changing any of them in Rust is a decision to record in `docs/rust-parity.md`, not an accident.

## routing/

One file per profile (`profile_ref` as in `contract/`). Each case is one TCP connection:

```json
{ "name": "tun_private_with_sni_stays_direct_ip",
  "inbound": "tun",
  "routing_mode": "rules",
  "host_ipv6_route": true,
  "destination": "192.168.1.10:20009",
  "sniff": { "tls_server_name": "intranet.video.example" },
  "reject_reason": "…" }
```

- **`inbound`** says how the connection reaches the core. Each desktop instance is built the way the product builds it:

  | `inbound` | Desktop instance | How the connection arrives |
  | --- | --- | --- |
  | `tun` | TUN-only instance | `destination` is an IP; `sniff` sends a TLS ClientHello or an HTTP request after the connection opens. |
  | `proxy-routed` | standard instance (local proxy, no TUN) | the local proxy's routed user |
  | `proxy-node:<id>` | standard instance | that node's user |
  | `system-proxy` | standard instance | the unauthenticated system proxy listener |

- **`routing_mode`** is `rules` (the default) or `global`.
- **`host_ipv6_route`** applies to the TUN only. `false` is a host whose IPv6 stack has no IPv6 path; there, direct traffic to a global IPv6 address is handed its sniffed domain over IPv4.
- **`reject_reason`** is a reader's annotation. The engine does not report which rule rejected a connection, so the expectation is only `REJECT`. The reasons used are:
  - the TUN's own subnet;
  - the fake-ip range without a known domain;
  - a profile rule.

  The Rust engine should reject for the same reason.

`expect[i]` is the decision for `cases[i]`:

| Field | Meaning |
| --- | --- |
| `action` | `DIRECT`, `PROXY` or `REJECT` |
| `node_id` | the node for `PROXY`: the selected node, a node named by a rule, or the user's node |
| `target`, `target_kind` | what the chosen outbound is asked to reach: a sniffed domain handed to a node, or the IP; `ip` or `domain` |
| `ipv6_hand_off` | the IPv6 hand-off happened (no IPv6 path, global IPv6 destination, known domain) |
| `classifier` | the flow-adapter classifier's decision for the same flow (`routing.Compile`), for comparison only |

The classifier has no client floor, no fake-ip or TUN-subnet rejection and no rule sets, so it disagrees with the engine on purpose in those cases. The engine's decision is the baseline.

How Go runs it: every outbound is bound to the loopback interface, so dials fail at once after routing has decided, and nothing leaves the host. The decision is read from the connection log line, written when routing picked an outbound; a connection closed without one was rejected. Internal outbound tags (sing-box names) are mapped to the fields above and never appear in the file.

UDP, DNS hijacking and the reverse mapping (domains learned from the core's own DNS answers) are not in these files.
