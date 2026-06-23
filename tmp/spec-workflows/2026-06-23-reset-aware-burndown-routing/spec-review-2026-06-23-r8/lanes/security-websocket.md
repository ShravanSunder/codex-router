# R8 Lane: Security + WebSocket + Affinity Redaction

Verdict: needs revision
Agent: Copernicus

## Candidate Findings

### Important: `affinity_key_hash` is security-load-bearing but underspecified

- Evidence: durable owner lookup depended on `hash("previous_response_id:<value>")`
  without naming algorithm, encoding, truncation, keyedness, or collision
  behavior.
- Failure path: implementation could reuse an unstable or truncated
  non-cryptographic hash and still pass simple no-raw-id tests.
- Refinement input: specify deterministic domain-separated collision-resistant
  digest, or keyed HMAC if previous-response ids are not assumed high-entropy;
  specify encoding/length and collision ambiguity fail-closed behavior.

## What Held

- WebSocket first-frame preselection remains constrained to top-level `type` and
  `previous_response_id` before selection.
- Owner-record creation is downstream of account selection/pinning.
- Owner writes are allowlisted to upstream response id fields and forbid raw
  bodies, frames, prompts, tool args, and raw previous ids.
- Local JSON `account_id` is separated from shared artifact redaction.
- The WebSocket failure matrix preserves zero side effects.

Completion receipt: answered with anchors.
Confidence: high
