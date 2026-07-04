# Quota Status Reset-Pace UX Plan Review

Date: 2026-07-04
Review target: `docs/plans/2026-07-04-quota-status-reset-pace-ux/implementation-plan.md`
Verdict: needs revision

## Coverage

- Plan file: 406 lines, read fully in chunks `1-160`, `161-320`, `321-406`.
- Ledger file: 67 lines, read fully.
- Lane source artifacts read:
  - `lanes/codebase-boundary.md`
  - `lanes/validation-proof.md`
  - `lanes/security-reliability.md`
- Accepted source artifact: none. The source is the chat-only accepted design summarized in the plan.
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
| whole-plan-cohesion | Codex subagent `019f2ac2-95ce-7a32-ac17-4f19f65515c9` | answered | needs revision |
| testability-validation | Codex subagent `019f2ac2-caf1-7fc3-ad36-730a02f3480b` | answered | needs revision |
| architecture-assumptions + execution-scope | Codex subagent `019f2ac2-feeb-7191-aea8-b715b96667b0` | answered | needs revision |
| security-reliability | Codex subagent `019f2ac3-4129-7d02-9c0d-709b6d8d3d8e` | answered | needs revision |

No external model lane was requested or run.

## Accepted Findings

### Blocker: 15-minute freshness can change runtime routing authority

The plan currently says 15 minutes is fresh/projectable and also says future refresh writes should use the 15-minute stale-after ceiling. Live code writes `DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS = 300` into selector state from `quota.rs`, selector reads mark windows stale based on that persisted timestamp in `sqlite.rs`, and runtime/proxy selection consumes that selector evidence.

Failure scenario: an executor changes persisted selector stale-after from 300 seconds to 900 seconds to satisfy the UX plan, and `serve` keeps routing on quota evidence that used to be stale. That changes selection behavior, not just `quota status` confidence display.

Smallest revision: split the requirement into two explicit thresholds:

- status/sample-confidence freshness for `quota status`
- selector-authority freshness for runtime routing

Then choose one path:

- keep selector stale-after at 300 seconds and add a status-only 900-second sample classifier; or
- explicitly expand scope to runtime routing semantics and add proxy/selection/state proof for 15-minute authority.

Required proof:

- If status-only: prove a 301-second persisted selector window is still stale for runtime selection, while `quota status` still shows values as aging/value-bearing.
- If routing changes too: prove selection behavior at 299s, 300s, 899s, 900s, and 901s in state/proxy/selection tests.

### Important: current dirty product diffs must be classified before implementation

Current worktree has product diffs in:

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-cli/src/presentation/quota.rs`

The diff swaps `terminal_size` for `crossterm` in the exact presentation file the plan intends to edit. The plan only says to re-check status, but not whether this diff is accepted baseline, unrelated blocker, or part of the quota UX scope.

Smallest revision: Gate 0 must classify these diffs. If accepted, the plan should name the `crossterm` terminal-width change as baseline to preserve and include it in proof. If unrelated, implementation should stop before editing overlapping files.

### Important: reset-pace meter needs a typed view-model boundary

The plan requires center-origin fill, active state color, and non-color meaning, but the current view model passes `burn_meter` and `weekly_pace` as raw strings and the renderer colors the whole pace line as one text unit.

Failure scenario: Slice 2 emits a glyph string, Slice 3 has to parse it or cannot color the center marker/state correctly, and parallel execution becomes unsafe.

Smallest revision: add a pre-slice contract step for a typed burn view model, with fields for state, multiple label, unavailable reason, meter segments or marker, and non-ANSI label. Slice 2 owns math/classification; Slice 3 owns rendering.

### Important: output contract is too open

The plan leaves exact sample labels and JSON freshness additions open while also requiring positive and negative string assertions. It must decide where banned old phrases apply.

Smallest revision:

- Define exact human labels and age bands.
- State whether `needs refresh` removal applies to table/plain only or also JSON.
- Prefer preserving JSON enum meanings unless a new field is explicitly named.
- Add JSON compatibility or new-field tests accordingly.

### Important: stale-value proof must be tied to no-provider-I/O proof

The plan requires stale values to remain visible and status to avoid provider I/O, but it does not require one durable fixture proving both at once.

Smallest revision: add a named integration proof such as `quota_status_preserves_stale_selector_window_values_without_provider_io`.

Required fixture:

- stale persisted 5h and weekly selector windows
- old 300-second stale-after state
- status command path without refresh/provider access
- assertions that values render with compact sample status
- assertions that no raw account IDs, secrets, or provider output appear

### Important: 15-minute projection boundary needs direct state proof

The plan asks for 14m59s, 15m00s, and 15m01s proof, but current `selection_projection.rs` only has the 300-second constant and read-only purity tests.

Smallest revision: add a focused projection test for latest-history ages 899, 900, and 901 seconds, or explicitly remove projection semantics from the 15-minute requirement if the decision is status-only display.

### Important: visual proof needs named acceptance cases

The plan says visual capture should cover fresh, stale, and degraded cases, but the existing ignored capture test currently writes normal-width captures plus a blocked case.

Smallest revision: require named fresh/stale/degraded capture artifacts and a manual checklist:

- center-origin meter
- under/healthy/over labels
- `burn unavailable` for unknown
- compact sample age
- no repeated banned phrases
- non-ANSI meaning remains visible

### Important: full validation gate is incomplete

The plan's full gate only names package tests and a vague fallback. CI runs:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace`
- `cargo deny check`
- `cargo audit`
- workflow lint through `actionlint`

Smallest revision: replace the vague fallback with explicit CI-equivalent gates, while allowing the executor to report environment blockers separately.

### Important: account-label safety assumption is false as written

The plan assumes existing account labels are safe. Live quota status builds status rows from raw `account.label()` and serializes that label as `safe_account_label`, while a real `safe_account_label(...)` helper already exists.

Smallest revision: either explicitly keep local labels as intentionally user-visible, or require quota status to derive a safe display label before human/plain/TUI/JSON output. If safe-labeling is in scope, add fixtures for email-like, token-like, and account-id-like labels.

### Important: degraded-read proof is too implicit

The plan says degraded fallback remains visible/tested, but it does not require proof that stale values can render while degraded data is not promoted into preferred routing authority.

Smallest revision: add a degraded-read fixture where projection fails but stale value-bearing windows render. JSON should assert `route_result = degraded` and `preferred_next_account_hash = null`; human/TUI/plain should show no preferred/selected authority.

### Important: telemetry/sample-age proof needs exact forbidden labels

If implementation adds sample freshness telemetry, the plan must require bucketed labels only and forbid exact ages or raw text.

Smallest revision: extend the telemetry contract test with required bucket labels and forbidden labels such as `sample.age_seconds`, `sample.age_text`, `provider.error`, and `account.label`.

### Nit: task sequence routes to the wrong review after implementation

The task sequence currently says to route to `plan-review-swarm` after full validation. This review is the pre-execution plan review. After implementation proof, the next workflow should be `implementation-review-swarm`.

## Rejected Or Deferred Findings

- No code implementation findings were accepted because this was plan review only.
- No plan edits were applied in this review; accepted blocker and important findings should route back through `plan-creation-swarm`.
- The exact product decision for 15-minute routing authority remains unresolved. The next plan pass must either keep routing freshness at 300 seconds or explicitly expand routing behavior scope and proof.

## Verdict

The plan is directionally aligned with the user's UX intent, but it is not ready for `implementation-execute-plan`.

Required route: `shravan-dev-workflow:plan-creation-swarm` to revise the plan, then `shravan-dev-workflow:plan-review-swarm` again before implementation.

phase_result: needs_revision
evidence: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-plan-review.md`
recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`
recommended_transition_reason: Plan review found a blocker-level ambiguity between 15-minute status freshness and runtime routing authority, plus proof and scope gaps that must be fixed before implementation.
