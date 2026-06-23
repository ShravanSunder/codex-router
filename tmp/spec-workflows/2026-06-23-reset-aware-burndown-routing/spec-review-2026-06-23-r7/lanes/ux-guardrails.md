# R7 Lane: UX + Guardrail Codification

Verdict: needs revision
Agent: Plato

## Candidate Findings

### Important: raw JSON account ids conflict with redacted artifacts

- Evidence: explicit JSON may include raw `account_id`, while proof language
  required redaction of machine status and smoke transcripts.
- Failure path: implementation could either remove useful local JSON ids or
  leak them into persisted/shared proof artifacts.
- Refinement input: allow raw `account_id` only in local JSON stdout; require
  redaction/hash in logs, traces, smoke transcripts, PR evidence, review
  attachments, and other shared artifacts.

### Important: JSON contract lacks normative envelope

- Evidence: JSON fields were listed without nesting/cardinality/nullability.
- Refinement input: add a compact JSON skeleton with route-level fields,
  `weighted_candidates[]`, `accounts[]`, `window_slots`, and `windows`.

### Important: status proof lacks route-band noise and duplicate-row guardrails

- Evidence: account-centric human status was specified, but proof did not forbid
  route summary rows or duplicate logical account rows.
- Refinement input: add structural assertions for one logical row per account,
  optional blank continuation line only, and no unrelated route-band rows/labels
  in default status.

## What Held

- Human table contract is substantially crisp: account-centric columns, 5h and
  weekly left/reset, routing phrase, next use, Unicode table bars, ASCII plain
  bars, no `pp`, no `bottleneck`, no raw score, and no default `account_id`.

Completion receipt: answered with anchors.
Confidence: high
