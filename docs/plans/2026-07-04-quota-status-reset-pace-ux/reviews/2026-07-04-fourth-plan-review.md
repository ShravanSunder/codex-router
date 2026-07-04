# Quota Status Reset-Pace UX Fourth Plan Review

Date: 2026-07-04
Review target: `docs/plans/2026-07-04-quota-status-reset-pace-ux/implementation-plan.md`
Verdict: needs revision

## Coverage

- Plan file: 815 lines, read fully in chunks `1-220`, `221-440`, `441-660`, `661-815`.
- Ledger file: 135 lines, read fully.
- Goal details: 127 lines, read fully.
- Transition log: 6 JSONL events, parsed successfully.
- Prior review read:
  - `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-third-plan-review.md`
- Accepted source artifact: none. Source is accepted chat design plus accepted review findings summarized in the plan and ledger.
- Live anchors checked:
  - `crates/codex-router-cli/src/quota.rs`
  - `crates/codex-router-cli/src/presentation/quota.rs`
  - `crates/codex-router-state/src/selection_projection.rs`
  - `crates/codex-router-state/src/sqlite.rs`
  - `crates/codex-router-cli/src/sessions.rs`
  - `crates/codex-router-proxy/src/account_selection.rs`
  - `.github/workflows/ci.yml`

## Swarm Coverage

| Lane | Backend | Status | Verdict |
| --- | --- | --- | --- |
| whole-plan-cohesion | Codex subagent `019f2ae6-3b58-7d81-b528-2b45f81f5ee2` | answered | ready |
| testability-validation | Codex subagent `019f2ae6-820f-7aa3-b8ea-d017ebfd4a5d` | answered | ready |
| architecture-assumptions + execution-scope | Codex subagent `019f2ae6-d44b-7b40-9bc7-60d4c99a9711` | answered | ready |
| security-reliability | Codex subagent `019f2ae7-1ff4-73a1-b37a-cc66ec7ef372` | answered | needs revision |

No external model lane was requested or run.

## Parent Synthesis

The revised plan resolves the third-review proof gaps: it now requires an
adversarial renderer/source-guard proof and an explicit per-case
ANSI/non-ANSI visual artifact matrix.

The remaining issue is a telemetry proof boundary. The plan states a broad
telemetry invariant, but its proof text can still be read as OTel metrics only.
Live code has both `tracing::info!` status attributes and OTel metric labels, so
the plan must require both surfaces to stay scrubbed and low-cardinality.

## Accepted Findings

### Important: telemetry proof must cover tracing/log attributes, not only OTel metrics

The plan forbids exact sample ages and raw sample strings in telemetry and maps
R15 to a telemetry label test. The live code has both status tracing attributes
and OTel metric labels:

- `crates/codex-router-cli/src/quota.rs` status tracing emits
  `codex_router.quota_status_selection`.
- `crates/codex-router-cli/src/quota.rs` emits OTel labels in
  `emit_quota_status_metrics(...)`.
- The existing telemetry contract test currently scopes itself to the metrics
  helper body.

Failure scenario: implementation adds `sample.age_seconds`, raw sample text,
provider error text, raw account label, or raw account ID to a tracing field
while keeping OTel metric labels clean. The metrics-only proof passes, but the
telemetry invariant is broken.

Smallest revision:

- Define telemetry as both OTel metrics and tracing/log attributes for this
  status surface.
- Revise R15 and Slice 4 proof text so the guard covers label names and emitted
  values/source fields across both surfaces.
- Forbid exact sample ages/text, provider errors, `account.label`, raw account
  IDs, and unsafe labels in both tracing/log attributes and metric labels.

Required proof:

```text
cargo test -p codex-router-cli quota_status_telemetry_contract_uses_scrubbed_low_cardinality_labels -- --exact
```

The test may be updated or paired with an equivalent source/fixture guard, but
it must cover status tracing/log attributes plus OTel metrics.

## Non-Blocking Questions

- If `sessions.rs` changes during implementation, add or run a sessions
  read-only regression. If it remains an untouched read-only anchor, code
  inspection plus existing query-only proof is sufficient.
- If proxy selection production code or proxy selection tests become touched,
  promote the optional proxy-level runtime-consumption guard to required.
- If reset-pace/sample DTO work makes the already-large `quota.rs` or
  `presentation/quota.rs` harder to review, the executor may introduce a narrow
  helper module under the same ownership and proof gates.

## Rejected Findings

- Whole-plan cohesion raised no accepted readiness blockers.
- Testability-validation raised no accepted readiness blockers.
- Architecture/execution scope raised no accepted readiness blockers.

## Verdict

The plan is close, but not ready for `implementation-execute-plan` until the
telemetry proof boundary is revised.

Required route: `shravan-dev-workflow:plan-creation-swarm` for the telemetry
proof revision, then `shravan-dev-workflow:plan-review-swarm` again before
implementation.

phase_result: needs_revision
evidence: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-fourth-plan-review.md`
recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`
recommended_transition_reason: Plan review found an important telemetry proof-boundary gap: status telemetry guards must cover tracing/log attributes as well as OTel metrics.
