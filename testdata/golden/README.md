# Golden files

Behaviour baselines exported from the Go core (frozen at v0.5.21), for the
Rust `ppvpn-engine` to be checked against (#45). Every file is
language-independent JSON; Go checks itself against the same files, so the
baseline cannot drift.

| Path | What | Go runner |
| --- | --- | --- |
| `options.json`, `options-multi-ingress.json` | sing-box options built from `testdata/profiles/*` | `internal/config/golden_test.go` (sing-box specific; not a Rust baseline) |
| `contract/` | Core API v1 contract: request sequences and the responses and events they produce | `api/golden_contract_test.go` |
| `routing/` | routing decisions | (later PR) |

Regenerate after an intended behaviour change, then review the diff:

```sh
go test ./api -run TestGoldenContract -update
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
