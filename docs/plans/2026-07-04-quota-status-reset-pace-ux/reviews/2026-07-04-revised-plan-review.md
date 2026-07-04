# Quota Status Reset-Pace UX Revised Plan Review

Date: 2026-07-04
Review target: `docs/plans/2026-07-04-quota-status-reset-pace-ux/implementation-plan.md`
Verdict: needs revision

## Coverage

- Revised plan file: 650 lines, read fully in chunks `1-160`, `161-320`, `321-480`, `481-650`.
- Ledger file: 88 lines, read fully.
- Goal details: 105 lines, read fully.
- Prior review: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-plan-review.md`, read fully.
- Source artifact: none. The accepted source is the chat design summarized in the revised plan plus accepted findings from the prior plan review.
- Live code anchors checked:
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
| whole-plan-cohesion | Codex subagent `019f2ad2-2c79-7822-bb5f-91a95f563213` | answered | needs revision |
| testability-validation | Codex subagent `019f2ad2-30f6-7b11-a941-de6cba03da09` | answered | needs revision |
| architecture-assumptions + execution-scope | Codex subagent `019f2ad2-351f-7512-b2ff-5277994f2cac` | answered | needs revision |
| security-reliability | Codex subagent `019f2ad2-3952-7610-b7ba-b51fd49b2db3` | answered | ready |

No external model lane was requested or run.

## Parent Synthesis

The revised plan resolves the original blocker-level product ambiguity: 15 minutes is now status-display/sample-confidence only, while runtime selector authority remains at the existing 300-second stale-after boundary unless a separate routing spec changes it.

However, the revised plan is not ready for implementation. The remaining issues are plan/proof issues, not product-design objections. They can be resolved in another plan-creation pass without product code changes.

## Accepted Findings

### Important: runtime-authority guard is aimed at the wrong layer

The plan requires proof that runtime selector authority remains 300 seconds, but the concrete runtime-authority gate points at `codex-router-selection`.

Live code evidence:

- `crates/codex-router-cli/src/quota.rs:83` defines `DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS = 300`.
- `crates/codex-router-cli/src/quota.rs:1054-1055` writes `observed + DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS`.
- `crates/codex-router-state/src/sqlite.rs:3138-3142` marks selector windows stale during selector input loading.
- `crates/codex-router-state/src/sqlite.rs:4326-4333` treats windows as stale when `now_unix_seconds >= stale_after_unix_seconds`.
- `crates/codex-router-proxy/src/account_selection.rs:997-1024` consumes projected selector state before burn-down selection.
- `crates/codex-router-selection/src/burn_down.rs` receives already-marked stale facts and cannot prove the persisted 300-second boundary by itself.

Failure scenario: an executor changes persisted `stale_after_unix_seconds` from 300 seconds to 900 seconds, but a selection-only stale-penalty test still passes because it never exercises SQLite stale marking.

Smallest revision:

- Replace or supplement the placeholder `codex-router-selection stale_selector_windows_remain_penalized_after_300_seconds` gate with a state/projection fixture.
- The fixture should build persisted selector windows with `stale_after = observed + 300` and prove eligible at 299 seconds and stale at 300/301 seconds.
- If test edits are required outside CLI files, the plan must list exact test-only write surfaces, or name existing exact tests and keep those modules read-only.

Required proof:

```text
cargo test -p codex-router-state <exact_300s_selector_stale_boundary_test> -- --exact
```

Optional additional proof:

```text
cargo test -p codex-router-proxy <exact_runtime_selector_uses_stale_marked_windows_test> -- --exact
```

### Important: sample confidence lacks an exact age source

The plan defines `fresh`, `stale`, and `unknown`, but does not say which timestamp owns `sample_age_seconds`.

Live code has multiple age concepts:

- Displayed quota windows carry `observed_unix_seconds` in `crates/codex-router-cli/src/quota.rs:1960-1968`.
- Selector-window display maps that observed time in `crates/codex-router-cli/src/quota.rs:1971-1978`.
- Snapshot fallback maps that observed time in `crates/codex-router-cli/src/quota.rs:1983-1994`.
- Refresh status is separate row metadata built from `QuotaRefreshStatusView` in `crates/codex-router-cli/src/quota.rs:1418-1421` and formatted in `crates/codex-router-cli/src/quota.rs:2194-2231`.
- Selector stale status is a runtime authority marker, not the display sample-age source.

Failure scenario: implementation labels a row `sample fresh` because refresh status is recent while the displayed quota value is older, or labels it stale at 301 seconds by reusing selector stale status. Either path reintroduces the stale/fresh confusion the UX is meant to remove.

Smallest revision:

- Define `sample_age_seconds` as derived from value-bearing displayed quota windows.
- Do not derive it from selector stale-after, selector status, or refresh status.
- For multiple displayed windows, choose the conservative oldest value-bearing observed age unless the product design explicitly chooses per-window metadata.
- Define `unknown` as no value-bearing observed sample.

Required proof:

- Unit tests with 5h and weekly windows whose observed ages differ.
- Boundary tests at 899, 900, and 901 seconds.
- A fixture where refresh status is fresh but a displayed value-bearing quota window is stale.

### Important: Slice 1 and Slice 2 are not parallel-safe around the shared DTO/view-model contract

The revised plan says Slice 1 and Slice 2 can start in parallel after Gate 0, but both slices touch the same row and presentation mapping contract.

Live code evidence:

- `quota_status_view_model` builds `weekly_window`, `burn_meter`, and `weekly_pace` together in `crates/codex-router-cli/src/quota.rs:1539-1555`.
- `quota_selected_account_view_model` builds selected account burn/window text in `crates/codex-router-cli/src/quota.rs:1562-1594`.
- `QuotaStatusRow` currently stores string-heavy row data plus `weekly_pace` in `crates/codex-router-cli/src/quota.rs:1859-1892`.
- Current presentation structs expose string fields for `burn_meter`, `weekly_pace`, and selected burn data in `crates/codex-router-cli/src/presentation/quota.rs:25-52`.

Failure scenario: one implementer changes stale sample fields while another changes reset-pace fields, both touching `QuotaStatusRow` and `QuotaStatusViewModel`; the merge either reintroduces string parsing or loses one contract.

Smallest revision:

- Insert a serialized shared status DTO/view-model contract step immediately after Gate 0.
- Slice 1 and Slice 2 may write pure helper tests in parallel only after that contract is fixed.
- Integration through `QuotaStatusRow` and `QuotaStatusViewModel` should be single-owner or sequential.

Required proof:

- Compile plus tests proving sample confidence and reset-pace are carried as typed fields into presentation without parsing display strings.

### Important: typed reset-pace ownership is semantically defined but not module-defined

The plan correctly requires a typed reset-pace model, but it does not define which module owns the DTO type versus math/classification versus rendering.

Live code evidence:

- Presentation structs are currently strings in `crates/codex-router-cli/src/presentation/quota.rs:25-52`.
- The row renderer colors the whole pace line in `crates/codex-router-cli/src/presentation/quota.rs:392-417`.
- The selected-account renderer colors the whole current burn line in `crates/codex-router-cli/src/presentation/quota.rs:475-477`.

Failure scenario: Slice 2 emits a pre-colored or pre-glyph string from `quota.rs`; Slice 3 cannot color the center marker or preserve non-ANSI semantic labels without parsing that string.

Smallest revision:

- State the module boundary explicitly. For example:
  - reset-pace DTO/view-model types live in `crates/codex-router-cli/src/presentation/quota.rs`, or in another named presentation-facing module;
  - construction/classification helpers live in `crates/codex-router-cli/src/quota.rs`, unless a named helper module is introduced;
  - presentation renders typed fields but does not compute burn math or parse glyph strings.

Required proof:

- Pure reset-pace model tests for state, segments, center marker, semantic label, and unavailable state.
- Presentation tests proving ANSI and non-ANSI rendering from typed fields.

### Important: CI-equivalent proof gate omits workflow lint

The revised plan says it incorporated the prior CI-equivalent finding, but the full gate and R18 list only Rust/dependency commands.

Live CI evidence:

- `.github/workflows/ci.yml:29-42` runs fmt, clippy, nextest, deny, and audit.
- `.github/workflows/ci.yml:44-52` also runs workflow lint through `raven-actions/actionlint@v2`.

Failure scenario: executor reports local "full CI-equivalent" proof, then PR CI fails in the workflow-lint job.

Smallest revision:

- Add workflow lint to R18 and the final CI-equivalent gate.
- If local `actionlint` is unavailable, require the exact missing-tool/environmental blocker to be reported separately from Rust/dependency gate status.

Required proof:

```text
actionlint .github/workflows/ci.yml
```

or an explicitly documented repo-local equivalent / exact unavailable-tool result.

### Question: visual/manual cases should name reset-pace state coverage

The plan requires fresh, stale, degraded, and unavailable-burn captures, and separately requires under/healthy/over reset-pace behavior. It does not explicitly compose those into named visual cases.

Failure scenario: generated captures cover fresh/stale/degraded data but only one burn state; text tests pass while the ANSI center marker/state color is never inspected.

Smallest revision:

- Name the capture matrix explicitly, for example:
  - `fresh-healthy`
  - `stale-under`
  - `degraded-over`
  - `unavailable-burn`
- Require representative narrow and sidecar widths.
- Require ANSI and non-ANSI inspection when proving color and semantic text.

Required proof:

- Ignored capture test writes the named artifacts.
- Manual checklist maps each filename to center-origin fill, semantic label, ANSI active marker color, and no overlap.

### Deferred Question: Gate 0 should still classify the current `crossterm` diff

The current worktree already contains a `terminal_size` to `crossterm` diff in:

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-cli/src/presentation/quota.rs`

The plan can either keep Gate 0 as-is or, if the next planning pass wants to reduce ambiguity, declare it an accepted baseline to verify and preserve. Parent review did not accept or reject that product diff; it remains a Gate 0 implementation decision unless the next planning pass chooses otherwise.

## Rejected Findings

- The security/reliability lane found no accepted blocker or important findings. The revised plan preserves no-provider-I/O, read-only observer, redaction, low-cardinality telemetry, and degraded-read authority boundaries at the plan level.
- No product-code implementation findings were accepted. This was a read-only plan review.

## Verdict

The revised plan is directionally right and much closer, but not ready for `implementation-execute-plan`.

Required route: `shravan-dev-workflow:plan-creation-swarm` to revise the plan, then `shravan-dev-workflow:plan-review-swarm` again before implementation.

phase_result: needs_revision
evidence: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-revised-plan-review.md`
recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`
recommended_transition_reason: Revised plan still has proof-layer and execution-scope gaps around runtime-authority testing, sample-age source, shared DTO sequencing, reset-pace module ownership, visual coverage, and workflow-lint proof.
