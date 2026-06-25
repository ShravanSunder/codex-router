# Research Ledger: Router Burndown, Quota Safety, Sessions

Date: 2026-06-25
Mode: read-only research synthesis

## Lane Summary

### quota-history-burndown-current-state

Result: current burn-down is point-in-time reset-aware scoring, not a historical run-rate model.

Evidence:
- `crates/codex-router-selection/src/burn_down.rs` owns 5h/weekly reset-aware scoring using current `QuotaWindowFact` inputs.
- `crates/codex-router-state/src/sqlite.rs` stores current rows in `quota_snapshots` and `selector_quota_windows`; both overwrite by account/route/window keys.
- `crates/codex-router-selection/src/reservation.rs` has an in-memory `ReservationBook`, but repo-backed selection does not currently use it for active load.
- `crates/codex-router-cli/src/quota.rs` renders pace/burn from current reset geometry, not observed historical slope.
- SQLx exists in the workspace and state crate through `AsyncSqliteStateStore`, while sync rusqlite paths still exist for CLI/state repositories.

Spec implications:
- Add append-only quota history with one-week retention.
- Add an estimator for observed quota burn rate.
- Add active load/reservation accounting for running Codex sessions.
- Keep reset credits visible first; only route on credits if explicitly specified later.

### codex-ws-quota-safety-lifecycle

Result: Codex quota errors are terminal for a turn; transport close can trigger reconnect, but it is not transparent enough to be the main quota contract.

Evidence:
- Codex `ModelClientSession` is turn-scoped, but the physical WebSocket can be cached across turns through `cached_websocket_session`.
- Codex replays `x-codex-turn-state` within a turn and clears it for the next logical turn, while tests show one physical WebSocket can carry multiple logical turns.
- `websocket_connection_limit_reached` is a special retryable WebSocket error path.
- Normal `usage_limit_reached` maps to `CodexErr::UsageLimitReached`, which is non-retryable and stops the turn after rate-limit updates.
- Router currently selects one account when opening a local WebSocket/upstream pair and then pumps bytes; no safe mid-stream account hot-swap exists.

Spec implications:
- Router must prevent known exhausted accounts before upstream selection.
- Router must not rely on Codex seeing usage-limit errors and recovering.
- Account identity can change only at a new upstream connection boundary.
- Proactive close/retire is allowed only with explicit installed-Codex proof that retry reconnects to a non-exhausted account and does not force sticky HTTP fallback.

### sessions-picker-cli-state-schema

Result: router needs a new command surface and should read Codex state metadata only.

Evidence:
- `crates/codex-router-cli/src/lib.rs` has manual parsing and no `sessions` command.
- Existing quota table/JSON output already uses `comfy-table` and `serde_json`.
- Codex CLI uses `clap`; upstream resume supports `session_id`, `--last`, `--all`, and `--include-non-interactive`.
- Codex state DB filename is `state_5.sqlite`; thread metadata contains id, rollout path, created/updated/recency, source, thread source, agent fields, provider, model, cwd, title/preview, archive, and git data.
- Codex rollout transcript search can inspect content; router sessions V1 should not use that path.

Spec implications:
- Add `clap` and `inquire` for router sessions V1.
- Default filters: scope=worktree, provider=any, source=interactive, sort=updated.
- Read metadata from Codex state DB, not transcript content.
- Launch selected sessions with `codex --profile codex-router resume <SESSION_ID>`.

## Parent Research Findings

- Workspace dependencies already include Tokio, Hyper, tokio-tungstenite, SQLx, comfy-table, serde_json, and rusqlite.
- CLI currently lacks clap/inquire and still has a custom parser.
- Existing async router/runtime work is separate from this follow-up; this follow-up must not reintroduce custom WebSocket protocol behavior.
- Old reset-aware burn-down spec made forecasting/history non-goals. The new spec explicitly supersedes that boundary.

## Open Questions Carried To Spec

1. Whether reset credits remain display-only or become routing input.
2. Whether proactive WebSocket retirement can be proven safe enough for automatic use before quota reaches zero.
3. Exact definition of sessions provider `current`.
4. Whether sessions title/preview should be displayed by default given they may contain prompt-derived text.

