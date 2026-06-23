# R10 Lane: Contract And Architecture

Status: answered
Verdict: needs revision
Agent: Hooke (`019ef55a-1c12-7231-ac31-de9fd25e4e60`)

Coverage:

- `reset-aware-burndown-routing-spec.md` was 1294 lines before R10 fixes.
- Read chunks: 1-260, 261-520, 521-780, 781-1040, 1041-1294.

Candidate findings:

1. Blocker: previous-response affinity named the desired record and HMAC
   behavior, but not the concrete owner/API boundary for hash-secret storage,
   hash construction, repository methods, and schema cutover.
2. Important: route-band policy was underspecified for non-`responses` route
   bands even though runtime selection is route-band generic.
3. Important: candidate ordering contradicted itself between neutral weighted
   order and later account-id ordering language.
4. Important: safe account label/redaction policy spanned CLI, proxy logs,
   traces, smoke transcripts, and JSON, but the shared sanitizer/hash owner was
   not named.

Parent reducer result:

- Accepted all four.
- Added core/secret-store/state/proxy affinity ownership and repository cutover
  APIs.
- Added a selection-owned v1 route-band policy registry for all currently
  classified route bands and fail-closed behavior for unregistered route bands.
- Split `accounts[]` order from `weighted_candidates[]` order.
- Named `codex-router-core::redaction` as the shared safe-label/hash owner.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-creation-swarm
