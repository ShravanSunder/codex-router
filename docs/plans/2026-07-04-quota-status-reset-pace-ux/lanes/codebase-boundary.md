# Lane: codebase-boundary

Status: answered
Reasoning effort: medium
Security context: applicable
Candidate evidence label: `quota-ux-freshness-boundary-scan`
Agent: `019f2aba-4422-74f3-be98-8da57a5095a8`

## Evidence Inspected

- `crates/codex-router-cli/src/quota.rs`
  - status dispatch and report construction
  - view-model shaping
  - refresh/window/pace helpers
  - quota status tests
- `crates/codex-router-cli/src/presentation/quota.rs`
  - TUI render and layout logic
  - row/detail rendering
  - width/reflow/focus tests
- `crates/codex-router-state/src/selection_projection.rs`
  - read-only projection path
  - burn-rate confidence/projection
  - read-only projection purity tests
- `crates/codex-router-state/src/sqlite.rs`
  - selector stale marking
  - read-only open and write surfaces
- `crates/codex-router-cli/src/sessions.rs`
  - reference read-only observer invariant

## Accepted Plan Constraints

- Primary write surface A: `crates/codex-router-cli/src/quota.rs`.
- Primary write surface B: `crates/codex-router-cli/src/presentation/quota.rs`.
- Conditional write surface C: `crates/codex-router-state/src/selection_projection.rs` for 15-minute projection freshness only.
- Avoid `crates/codex-router-state/src/sqlite.rs` unless a separate replan accepts a state-layer change.
- Do not touch provider refresh/write paths.
- Preserve `quota status` and `sessions` read-only observer boundaries.

## Risks

- Plain/table/json share helper text; scope changes carefully.
- Current view models are string-heavy; richer burn semantics may require small model expansion.
- Stale rows are currently human-demoted; display changes must not accidentally alter routing semantics.

## Parent Disposition

Accepted into implementation plan.

