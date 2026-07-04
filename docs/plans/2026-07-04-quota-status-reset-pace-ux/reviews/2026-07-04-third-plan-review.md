# Quota Status Reset-Pace UX Third Plan Review

Date: 2026-07-04
Review target: `docs/plans/2026-07-04-quota-status-reset-pace-ux/implementation-plan.md`
Verdict: needs revision

## Coverage

- Plan file: 772 lines, read fully in chunks `1-200`, `201-400`, `401-600`, `601-772`.
- Ledger file: 118 lines, read fully.
- Goal details: 118 lines, read fully.
- Transition log: 4 JSONL events, parsed successfully.
- Prior reviews read:
  - `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-plan-review.md`
  - `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-revised-plan-review.md`
- Accepted source artifact: none. Source is accepted chat design plus accepted review findings summarized in the plan and ledger.
- Live anchors checked:
  - `crates/codex-router-cli/src/quota.rs`
  - `crates/codex-router-cli/src/presentation/quota.rs`
  - `crates/codex-router-state/src/selection_projection.rs`
  - `crates/codex-router-state/src/sqlite.rs`
  - `crates/codex-router-cli/src/sessions.rs`
  - `crates/codex-router-proxy/src/account_selection.rs`
  - `crates/codex-router-selection/src/burn_down.rs`
  - `.github/workflows/ci.yml`

## Swarm Coverage

| Lane | Backend | Status | Verdict |
| --- | --- | --- | --- |
| whole-plan-cohesion | Codex subagent `019f2add-eaa4-7422-aaea-0f4081752083` | answered | ready |
| testability-validation | Codex subagent `019f2add-ef98-7f52-83b8-01766aaea9ec` | answered | needs revision |
| architecture-assumptions + execution-scope | Codex subagent `019f2add-f4dc-7700-a49b-ea41feecec0c` | answered | ready |
| security-reliability | Codex subagent `019f2add-f8ee-7270-a9fb-b500d722dd31` | answered | ready |

No external model lane was requested or run.

## Parent Synthesis

The plan now resolves the major second-review gaps: runtime authority proof is at the state/projection boundary, sample age is sourced from displayed value-bearing windows, DTO integration is serialized, reset-pace ownership is module-scoped, workflow lint is included, and the visual cases are named.

The remaining issues are proof sharpness issues:

- The renderer proof must be adversarial enough to catch string parsing shortcuts.
- The visual capture matrix must require paired ANSI/non-ANSI artifacts per named case and representative width.

These are plan-creation fixes. No product code should be implemented yet.

## Accepted Findings

### Important: shared DTO proof can still miss renderer string parsing

The plan requires typed reset-pace/sample DTOs and says presentation must not parse glyph strings, but the proof currently says only "compile + typed field tests" plus a named DTO test.

Live code evidence:

- `crates/codex-router-cli/src/presentation/quota.rs:25-52` currently exposes raw string fields for row/detail burn data.
- `crates/codex-router-cli/src/presentation/quota.rs:392-417` currently composes a pace line from string fields.

Failure scenario: implementation adds typed fields but the renderer still infers state or color by scanning `semantic_label`, `multiple_label`, or meter glyph text. Rendered-output assertions could still pass while the architectural boundary is broken.

Smallest revision:

- Require an adversarial renderer proof such as `presentation::quota::tests::quota_status_renderer_uses_reset_pace_fields_without_parsing_strings`.
- Require a source guard or adversarial fixture where typed state/segments drive rendering while labels contain sentinel/conflicting strings.
- Explicitly forbid renderer-side parsing of reset/sample display strings and meter glyphs, except in tests.

Required proof:

```text
cargo test -p codex-router-cli presentation::quota::tests::quota_status_renderer_uses_reset_pace_fields_without_parsing_strings -- --exact
```

### Important: visual capture matrix names states but not per-state ANSI/non-ANSI artifacts

The plan names `fresh-healthy`, `stale-under`, `degraded-over`, and `unavailable-burn`, and the checklist mentions ANSI/non-ANSI, but it does not require each named case to emit both styles at representative widths.

Failure scenario: all data cases are captured, but only one ANSI state is visually inspected. Under/over active-marker colors or non-ANSI labels can regress without a manual artifact catching it.

Smallest revision:

- Make the capture matrix explicit as `case x width x style`.
- Require each named case at one narrow width and one sidecar width.
- Require paired plain and ANSI artifacts for every named case/width pair.

Required proof:

Example artifact groups:

```text
fresh-healthy-48.txt
fresh-healthy-48.ansi
fresh-healthy-160.txt
fresh-healthy-160.ansi
```

and the same shape for `stale-under`, `degraded-over`, and `unavailable-burn`.

## Rejected Findings

- Whole-plan cohesion raised no accepted findings.
- Architecture/execution scope raised no accepted findings.
- Security/reliability raised no accepted findings.
- Gate 0 remains an intentional execution-time classification of the current `crossterm` dependency/presentation diff.

## Verdict

The plan is close, but not ready for `implementation-execute-plan`.

Required route: `shravan-dev-workflow:plan-creation-swarm` for the two proof revisions above, then `shravan-dev-workflow:plan-review-swarm` again before implementation.

phase_result: needs_revision
evidence: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-third-plan-review.md`
recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`
recommended_transition_reason: Plan review found remaining proof sharpness gaps for renderer string-parsing guardrails and per-case ANSI/non-ANSI visual artifacts.
