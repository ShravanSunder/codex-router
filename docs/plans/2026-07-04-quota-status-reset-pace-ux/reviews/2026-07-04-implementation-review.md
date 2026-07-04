# Quota Status Reset-Pace UX Implementation Review

Date: 2026-07-04
Goal id: `2026-07-04-quota-status-reset-pace-ux`
Workflow: `shravan-dev-workflow:implementation-review-swarm`
Mode: plan-backed implementation review after local implementation and proof.

## Verdict

ready

Reason: all accepted implementation-review findings were fixed in scope and the post-fix proof gates passed. No blocker, important, or decision-relevant open findings remain.

phase_result: complete
recommended_next_workflow: `shravan-dev-workflow:implementation-pr-wrapup`
recommended_transition_reason: Implementation review findings are addressed, proof is current for the fixed worktree, and the remaining lifecycle gate is PR creation/update and readiness proof.

## Reviewed Scope

- Plan: `docs/plans/2026-07-04-quota-status-reset-pace-ux/implementation-plan.md`
- Final plan review: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-final-plan-review.md`
- Implementation diff:
  - `Cargo.toml`
  - `Cargo.lock`
  - `crates/codex-router-cli/Cargo.toml`
  - `crates/codex-router-cli/src/lib.rs`
  - `crates/codex-router-cli/src/presentation/quota.rs`
  - `crates/codex-router-cli/src/quota.rs`
  - `crates/codex-router-state/src/sqlite.rs`
- Visual artifacts: `tmp/ux-proof/production/`
- Existing branch-baseline dependency/terminal-width diff was preserved and not reverted.

## Accepted Findings And Fixes

1. Degraded fallback leaked preferred-routing authority.
   - Fix: `QuotaStatusRow::normalize_degraded_projection_authority()` now clears `preferred_next`, maps preferred routing reasons to `UnknownFallbackAvailable`, and updates row routing/next-use state.
   - Proof: degraded JSON integration test now asserts no preferred authority leaks from degraded read fallback.

2. Selected details appeared without authoritative selection.
   - Fix: focused details render only when `view_model.selected` exists.
   - Proof: `quota_status_without_authoritative_selection_does_not_show_selected_details`.

3. ANSI output did not color reset-pace state.
   - Fix: ANSI writer applies a scoped post-pass to reset-pace segments after static `iocraft` rendering.
   - Proof: `quota_status_ansi_colors_reset_pace_by_state` and capture checks for green/yellow/red 256-color codes.

4. Narrow rows lost reset/sample semantics.
   - Fix: compact 48-column row mode preserves reset state and sample confidence with `healthy`, `under`, `over`, `burn unavailable`, `fresh`, `stale`, and `unknown`.
   - Proof: `quota_status_narrow_rows_preserve_reset_and_sample_semantics`; `fresh-healthy-48.txt` now carries `healthy` and `fresh`.

5. Plain/non-TTY output still used legacy `pace` / `burn`.
   - Fix: plain header is now `account status 5h weekly reset pace sample updated clients resets available routing next use`, with typed reset/sample summaries instead of old row strings.
   - Proof: updated plain-output integration assertions.

6. Row sample metadata used hidden 5h data when list row displayed weekly.
   - Fix: row view model derives sample metadata from the weekly display window only; selected details/plain still use displayed 5h + weekly windows.
   - Proof: `quota_status_row_sample_uses_only_weekly_window_age`.

7. Stale visual artifact contradicted itself.
   - Fix: stale capture fixture now uses a failed refresh age consistent with stale sample confidence.
   - Proof: regenerated `stale-under-160.txt` shows `refresh failed 15m 1s ago: network` and selected detail `sample stale 15m 1s`.

## Source Trace

review_class: plan-backed
source_coverage_state: covered
source_backed_verdict_attempted: true
whole-source-trace: completed
classifier_reason: the implementation is executing a reviewed plan with runtime-authority, read-only observer, stale-data display, and user-facing terminal UX obligations.

source/spec/plan/code/proof matrix:

| source_obligation_id | source_anchor | plan_anchor | implementation_anchor | proof_anchor | reachability_status | coverage_status | false_substitute_risk | accepted_deviation_bucket | accepted_route_target |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| R1/R3/R4 | user chat + plan | sample freshness/stale display/no-provider rows | `sample_metadata_from_display_windows`, `sample_metadata_from_display_window`, stale integration fixtures | focused `quota_status` tests and stale capture artifacts | live | covered | stale values could be hidden while status says stale | none | none |
| R2 | plan review blocker | runtime selector authority remains 300s | `crates/codex-router-state/src/sqlite.rs` persisted stale-after test | `selector_inputs_mark_windows_stale_at_persisted_300_second_boundary` | live | covered | 15m display threshold could silently become routing authority | none | none |
| R7/R8/R9 | user reset-pace design | typed reset-pace model and ANSI/non-ANSI rendering | `reset_pace_view_model_from_snapshot`, `reset_pace_view_model_from_multiple_basis_points`, `colorize_reset_pace_ansi` | presentation tests, ANSI capture checks | live | covered | labels could be parsed from glyph strings | none | none |
| R10/R11/R17 | visual/layout contract | narrow + sidecar visual matrix | compact row rendering and selected details gating | ignored capture generation and spot checks in `tmp/ux-proof/production/` | live | covered | narrow output could pass unit tests while losing meaning | none | none |
| R12/R13/R16 | observer/degraded authority | read-only observer and degraded fallback proof | degraded normalization and read-only projection tests | degraded integration test and projection purity tests | live | covered | degraded facts could masquerade as selectable authority | none | none |
| R14/R15 | redaction/telemetry guard | low-cardinality output/telemetry contract | existing scrubbed labels and status telemetry guards preserved | full workspace test and focused quota suites | live | covered | exact sample ages/provider text could leak in telemetry | none | none |

## Review Proof

- Implementation proof was checked against the reviewed requirements/proof matrix.
- Red/green evidence: behavior changes were covered by focused tests added or strengthened during implementation; full post-fix suite passed.
- Weakened or relabeled proof lanes found: none after review-finding fixes.
- External model lanes: not requested.
- Remaining proof gap: PR/CI readiness is not yet proven; route to `implementation-pr-wrapup`.

## Post-Fix Verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo nextest run --workspace`: pass, 584 tests passed, 14 skipped.
- `actionlint .github/workflows/ci.yml`: pass.
- `git diff --check`: pass.
- `cargo deny check`: pass after sandbox DB-lock retry with approval; duplicate-crate warnings only.
- `cargo audit`: pass after sandbox DB-lock retry with approval.
- `cargo test -p codex-router-cli quota::tests::quota_status_capture_artifacts_for_design_review -- --ignored --exact`: pass; regenerated visual artifacts.
- `cargo test -p codex-router-cli quota_status`: pass, 34 passed, 1 ignored.
- `cargo test -p codex-router-cli quota::tests::quota_status_`: pass, 22 passed, 1 ignored.
- `cargo test -p codex-router-cli presentation::quota::tests::quota_status_`: pass, 8 passed.
- `cargo test -p codex-router-state sqlite::selector_inputs_mark_windows_stale_at_persisted_300_second_boundary -- --exact`: pass.
- `cargo test -p codex-router-state selection_projection::tests::read_only_projection_`: pass, 2 passed.

## Visual Proof Notes

- `tmp/ux-proof/production/stale-under-160.txt` shows `refresh failed 15m 1s ago: network` and `sample stale 15m 1s`.
- `tmp/ux-proof/production/degraded-over-160.txt` shows `responses -> none [blocked] no selectable account`, with no selected-account authority.
- `tmp/ux-proof/production/fresh-healthy-48.txt` preserves compact `healthy` and `fresh` semantics.
- ANSI spot checks confirmed reset-pace state color output:
  - healthy green: `\x1b[38;5;10m`
  - under yellow: `\x1b[38;5;11m`
  - over red: `\x1b[38;5;9m`

## Routing Follow-Through

Accepted implementation-review findings were fixed in the implementation scope and proven. The next workflow is PR wrap-up: inspect local branch state, commit scoped files, push/open/update a PR, inspect checks/comments/review threads/mergeability, require a quiet poll, and stop before merge unless explicitly authorized.
