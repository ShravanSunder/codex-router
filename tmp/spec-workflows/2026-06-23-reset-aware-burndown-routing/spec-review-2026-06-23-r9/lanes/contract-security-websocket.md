# R9 Lane: Contract + Security + WebSocket

Verdict: needs revision
Agent: Ampere

## Candidate Findings

### Important: `router_affinity_hash_secret` lifecycle is underspecified

- Evidence: the spec defined HMAC algorithm and storage sensitivity, but not
  whether the secret is generated once, rotates with bearer/account credentials,
  persists across restarts, or invalidates existing owner rows when missing or
  replaced.
- Failure path: planning could rotate or regenerate the affinity hash secret
  with local bearer-token rotation, on every serve start, or during credential
  rotation, breaking durable continuation lookup.
- Refinement input: define generated-once-per-router-root lifecycle,
  independent of bearer/account credential rotation, no v1 rotation path, and
  fail-closed behavior if missing/unreadable/replaced.

## What Held

- HMAC algorithm/domain/encoding is precise enough.
- Shared helper, hard schema cutover, duplicate ambiguity fail-closed, owner
  `usable | reserve` eligibility, WebSocket first-frame allowlist, downstream
  owner-record creation, and redaction rules held.

Completion receipt: answered with anchors.
Confidence: high
