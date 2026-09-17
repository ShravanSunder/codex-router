# Host restart: fast kill, client reconnect, coherent CLI, progress UI

Owner decisions (Shravan, 2026-09-17). Worktree: `codex-router.feat-host-restart-reconnect`,
branch `feat/host-restart-reconnect`. Supersedes the "no client-connection continuity"
and "pinned upstream 60/70s shutdown" statements in `docs/specs/2026-09-13-host-restart/`.

## Why

The Codex TUI dials the app-server unix socket directly (Host is not in the data path) and
gives up reconnecting after ~15s: attempts at t=0/1/3/7/15s after disconnect, hard-coded
upstream (`codex-rs/tui/src/app/reconnect.rs`, `for delay in [0, 1, 2, 4, 8]`; ENOENT /
ECONNREFUSED fail instantly so the 120s deadline never binds). A fresh app-server
cold-resumes the thread from the rollout (`thread/resume` with only `threadId`); an
in-flight turn comes back `Interrupted`. So the only requirement for reconnect is: **the
app-server socket accepts and initializes again within ~15s of dropping clients.**
Today teardown waits up to 60s and the app-server is the last thing started.

## Slices

### S1 — Fast, clean app-server kill
- `crates/codex-router-host/src/app_server_shutdown.rs`: SIGTERM → wait 1s → SIGKILL.
  Total observation bound stays small (kill + short reap bound, e.g. 1s + 4s).
- SIGKILL targets the **process group** (`send_group_kill`), not only the PID: with a 1s
  grace, in-flight turns are routinely hard-killed and their children (MCP servers, tool
  shells) must not be orphaned. SIGTERM stays exact-PID.
- Rename/remove the "pinned upstream" constants, docs, and tests that assert 60/70.
  Update CLI deadlines in `crates/codex-router-cli/src/host_command/mod.rs`
  (`APP_SERVER_RESTART_DEADLINE`, restart 150s, the `> 70s` test) to the new bounds.
- A stale socket file left by SIGKILL must not block respawn: verify
  `require_unowned_app_server_endpoint` + upstream bind handle a dead socket path; fix at
  the endpoint-ownership boundary if not.

### S2 — App-server socket back ASAP
- Whole-Host restart and foreground startup: app-server spawn must not wait behind router
  start, schema export, or telemetry flush. Start router and app-server concurrently;
  move `prepare_schema` off the critical path (background after socket is up; it already
  degrades to "raw native access"). Keep `executable_identity` / version only if they
  are cheap — measure with `debug_readiness_timing`.
- Whole-Host replacement: app-server and router teardown may run concurrently.
- Proof: integration test holding a real client connection to the app-server socket
  across `host restart` and `host app-server restart` on fixture children; assert the
  socket is accepting again well inside 15s (target: < 5s gap). Extend
  `tests/runtime_restart.rs` / `tests/update_reexec.rs` patterns; no sleeps.

### S3 — Command shape (hard cutover, no aliases)
```
codex-router host                      foreground Host
codex-router host status
codex-router host restart              router + app-server + Host image
codex-router host router restart
codex-router host app-server restart
codex-router host app-server update
```
- Remove `restart-router`. Update clap enums, top-level help text, contract tests,
  `compiled_cli_host_acceptance.rs`, README/docs.
- `host router restart` must be a proper restart: bounded stop, respawn, readiness probe,
  clear result when the router is external (not Host-owned).

### S4 — Streaming progress + indicatif for every host command
- Protocol: extend `HostProgress` with phase events (stopping app-server, killed/forced,
  stopping router, re-exec, router ready, app-server ready, remote-control ready; update
  phases). `AwaitHostStart` streams startup phases.
- Client: `operator_client.rs` currently buffers to EOF — stream frames to a callback.
- Presentation (`crates/codex-router-cli/src/presentation/host.rs`): indicatif spinner per
  phase resolving to ✓/✗ with elapsed; plain line output when stdout is not a TTY.
  Replace `{:?}` dumps with human-readable status (no `desktop_relaunch:
  required_if_running`-style internals unless actionable).
- Same phase rendering for foreground `host` startup and `serve` startup.
- One presentation module owns indicatif; commands pass typed phase events, not strings.

### S5 — Host lifecycle telemetry reaches logs
- `codex_router.host.lifecycle` events (`lifecycle_telemetry.rs`) are absent from
  VictoriaLogs (14d scrape: zero records; only `managed_app_server` warnings and
  `child_diagnostic` arrive). Find why (level filter, target filter, or pre-exec
  shutdown dropping them) and fix. Emit phase timings from S2 so restart gaps are
  observable. Also: "native schema export unavailable" fires on every start — diagnose.

### S6 — SQLite event retention (separate audit first)
- `lifecycle-observation` journal already has retention. Audit `codex-router-state`
  tables that append events (`active_session_events`, `quota_history_observations`,
  `previous_response_affinity_owners`, `session_account_affinities`, rollups) for
  unbounded growth; report sizes + existing pruning before any change. Design returns to
  the orchestrator; no schema change without sign-off.

## Order and gates
S1 → S2 (reconnect fix, ships first) → S3 → S4 → S5. S6 audit runs independently.
Repo gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, repo test runner, sqlx prepare check if SQL changes. Acceptance on the
isolated debug Host only (port 18787); never restart the production Host.

## Note
`main` has unrelated uncommitted work touching `crates/codex-router-cli/src/telemetry.rs`;
this branch starts from committed `main`. Expect a merge touchpoint there in S5.
