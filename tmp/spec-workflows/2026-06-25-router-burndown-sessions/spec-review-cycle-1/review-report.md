# Spec Review Cycle 1

Date: 2026-06-25
Target: tmp/spec-workflows/2026-06-25-router-burndown-sessions/router-burndown-quota-safety-sessions-spec.md
Coverage: 248 lines read end-to-end before review.

## Lanes

- whole-spec-coverage: needs revision
- requirements-testability + validation-and-testability: needs revision
- contract-and-scope + architecture-boundaries + security-threat-model: needs revision

## Accepted Findings

1. SQLx-only was missing from the spec while the repo still has SQLx and rusqlite. Accepted as blocker. Revised the spec to require SQLx for all new or extended SQL access in this implementation, including sessions reads.
2. Estimator proof signals were too vague. Accepted. Revised the spec to define sample/confidence thresholds and projection confidence labels.
3. Active-load reservations lacked owner, unit, and release semantics. Accepted. Revised the spec to define proxy-owned handles, selector-consumed summaries, route-band units, deterministic weights, lifecycle, and stale cleanup.
4. Quota-error quarantine needed a parser boundary against pass-through behavior. Accepted. Revised the spec to allow only recognized provider control/error envelopes and forbid arbitrary payload parsing/quarantine.
5. Sessions title/preview privacy was ambiguous. Accepted. Revised the spec to allow sanitized/truncated human labels only; JSON/default search exclude them.
6. Live-gated E2E semantics were vague. Accepted. Revised the spec to require explicit opt-in, credential-unavailable exits, refresh/status/selection dry-run, and separate confirmation for generation/WebSocket live steps.

## Verification

- Parent re-read revised sections after patch.
- Plan was patched to carry the same accepted constraints before plan review.

phase_result: complete
evidence: tmp/spec-workflows/2026-06-25-router-burndown-sessions/router-burndown-quota-safety-sessions-spec.md
recommended_next_workflow: shravan-dev-workflow:plan-review-swarm
recommended_transition_reason: Spec has one completed review cycle and accepted findings have been applied for plan review.

