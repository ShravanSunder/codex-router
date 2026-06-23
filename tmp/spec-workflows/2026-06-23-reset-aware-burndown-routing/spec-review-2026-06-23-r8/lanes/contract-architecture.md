# R8 Lane: Contract + Architecture + Spec Difference

Verdict: needs revision
Agent: Lovelace

## Candidate Findings

### Important: affinity lookup key hashing is not exact enough

- Evidence: the spec used `affinity_key_hash = hash(...)`, while current code
  has raw-string affinity keys and unrelated non-contractual redaction helpers.
- Failure path: planning could choose raw keys, `DefaultHasher`, SHA-256, HMAC,
  or different encodings independently.
- Refinement input: specify stable algorithm, encoding, construction owner,
  persisted-column contract, and hard cutover from existing raw affinity rows.

### Question: owner-hit route eligibility needs exact availability mapping

- Evidence: owner-hit validity used `route-eligible`, while burn-down has
  `usable`, `reserve`, `unknown`, `blocked`, and `excluded`.
- Failure path: planning could allow `unknown` continuation owners or fail them
  closed, changing continuation reliability and quota safety.
- Refinement input: map owner validity to availability classes and prove
  unknown/stale/reserve behavior.

## What Held

- Account inclusion, selected-pool-only `weighted_candidates`, JSON envelope,
  proxy/state/selection ownership, and target implementation deltas were
  otherwise ready for planning.

Completion receipt: answered with anchors.
Confidence: high
