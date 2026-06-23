# R7 Lane: Contract + Architecture + Spec Difference

Verdict: needs revision
Agent: Hilbert

## Candidate Findings

### Blocker: previous-response affinity lacks owner-record creation contract

- Evidence: the spec defined durable owner lookup and fail-closed behavior, but
  not the write trigger, record shape, credential-generation staleness semantics,
  or allowed upstream response-id parsing.
- Failure path: planning would invent whether HTTP/SSE and WebSocket parse
  upstream payloads, whether credential generation is stored, and how redaction
  applies.
- Refinement input: define `PreviousResponseOwnerRecord`, pin-write triggers for
  HTTP/SSE and WebSocket, stored credential generation, allowlisted upstream id
  fields, and redaction/proof boundaries.

### Important: account assessment inclusion contradiction

- Evidence: same line-377 contradiction found by the other lanes.
- Refinement input: assess every supplied account row; filter selected pools
  after classification.

### Important: JSON schema is a field inventory, not a data contract

- Evidence: route-level and per-account fields were mixed in one bullet list.
- Failure path: a plan could invent incompatible JSON envelopes and test that
  invention.
- Refinement input: add a compact normative JSON object sketch with top-level
  route fields, `weighted_candidates[]`, `accounts[]`, per-account
  `window_slots.{5h,weekly}`, and per-account `windows[]`.

## What Held

- R6 ownership split mostly held: proxy owns fact adaptation/fairness/runtime
  exact selection; burn-down owns pure classification from supplied facts;
  credential resolution stays outside burn-down.

Completion receipt: answered with anchors.
Confidence: high
