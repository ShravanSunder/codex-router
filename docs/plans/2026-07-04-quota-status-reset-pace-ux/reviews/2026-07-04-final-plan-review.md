# Quota Status Reset-Pace UX Final Plan Review

Date: 2026-07-04
Review target: `docs/plans/2026-07-04-quota-status-reset-pace-ux/implementation-plan.md`
Verdict: ready

## Coverage

- Plan file: 829 lines after the fourth-review telemetry revision, read fully.
- Ledger file: 151 lines, read fully.
- Fourth review: 114 lines, read fully.
- Goal details: 135 lines, read fully.
- Transition log: 8 JSONL events, parsed successfully.
- Accepted source artifact: none. Source is accepted chat design plus accepted
  review findings summarized in the plan, ledger, and details.
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
| whole-plan-cohesion focused re-review | Codex subagent `019f2aec-b381-74a3-8733-5bb9d05bf36d` | answered | ready |
| security-testability telemetry re-review | Codex subagent `019f2aec-f770-77f0-a31d-b2914d8cce4a` | answered | ready |

Earlier fourth-review lanes are retained as evidence:

| Lane | Backend | Status | Verdict |
| --- | --- | --- | --- |
| whole-plan-cohesion | Codex subagent `019f2ae6-3b58-7d81-b528-2b45f81f5ee2` | answered | ready |
| testability-validation | Codex subagent `019f2ae6-820f-7aa3-b8ea-d017ebfd4a5d` | answered | ready |
| architecture-assumptions + execution-scope | Codex subagent `019f2ae6-d44b-7b40-9bc7-60d4c99a9711` | answered | ready |
| security-reliability | Codex subagent `019f2ae7-1ff4-73a1-b37a-cc66ec7ef372` | answered | needs revision before the telemetry fix |

No external model lane was requested or run.

## Parent Synthesis

The fourth-review telemetry gap is resolved. The plan now defines telemetry as
both status tracing/log attributes and OTel metrics, forbids exact sample
ages/text, raw provider errors, raw account IDs, raw account labels, and unsafe
labels across telemetry labels and values, and requires the existing telemetry
contract proof to be updated or paired with an equivalent guard covering both
surfaces.

The prior third-review proof gaps remain resolved:

- Renderer code must consume typed sample/reset-pace fields and must not parse
  reset/sample display strings or meter glyphs.
- The visual gate requires paired plain `.txt` and ANSI `.ansi` artifacts for
  each named case at narrow and sidecar widths.

The earlier routing and observer invariants remain explicit:

- 15 minutes is status-display/sample-confidence only.
- Runtime selector authority remains governed by persisted selector stale-after
  behavior at the existing 300-second boundary.
- Stale values still display in quota status.
- `quota status` and `sessions` stay read-only observers; `serve` owns writes.
- No provider I/O is added to status.

## Findings

No blocker or important findings remain.

Non-blocking execution notes:

- If `sessions.rs` changes during implementation, add or run a sessions
  read-only regression. If it remains an untouched read-only anchor, code
  inspection plus existing query-only proof is sufficient.
- If proxy selection production code or proxy selection tests become touched,
  promote the optional proxy-level runtime-consumption guard to required.
- If reset-pace/sample DTO work makes the already-large `quota.rs` or
  `presentation/quota.rs` harder to review, the executor may introduce a narrow
  helper module under the same ownership and proof gates.

## Verdict

Ready for `shravan-dev-workflow:implementation-execute-plan`.

Do not skip Gate 0 in the implementation plan: the current dirty
`crossterm`/terminal-width product diff must be classified before overlapping
implementation edits.

phase_result: complete
evidence: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-final-plan-review.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Plan review found no remaining blocker or important findings after the telemetry proof revision; implementation can begin from Gate 0.
