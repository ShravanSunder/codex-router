# Restructure Lane: Scope And Proof Fit

Lane: `scope-and-proof-fit`
Agent: Bohr (`019eeee0-33c0-7ed0-9317-92e8f50d35e8`)
Status: answered
Evidence label: `scope-and-proof-fit.plan-split.v1`
Security context: applicable
Confidence: high

## Candidate Evidence

- Plan 1A / Plan 1B is the right top-level split.
- Plan 1A is substrate work: credential truth, fail-closed mutation, redaction, shared runtime resolver, durable selector inputs.
- Plan 1B is runtime behavior: failure taxonomy, startup refresh, selection, next-path switching, SQLite status, smoke, docs, closeout.
- The previous failure was structural: the umbrella still looked executable and allowed partial work to look done.

## Accepted Parent Changes

- Umbrella status changed to non-executable.
- Plan 1A completion language now says stacked prerequisite/merge-gate, not final closeout.
- T1 is now explicit behavior-preserving extraction only where needed by T2-T5.
- QR-14/weekly selection correctness is owned by Plan 1B, while Plan 1A owns only durable selector input.
- WebSocket remains mandatory Plan 1B proof.

## Anchors

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:98-153`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:203-258`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:364-430`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/post-review-synthesis.md:24-50`

## Completion Receipt

Status: answered.
Parent wrote this lane artifact.
Remaining uncertainty: whether Plan 1A can be made separately mergeable later; current plan treats it as a stacked prerequisite inside this PR.
