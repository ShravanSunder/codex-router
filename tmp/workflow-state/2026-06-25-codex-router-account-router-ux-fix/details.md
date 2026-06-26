# 2026-06-25 codex-router account router UX fix

Goal id: `2026-06-25-codex-router-account-router-ux-fix`

Required workflow skill: `shravan-dev-workflow:orchestrator-goal`

Current workflow: `shravan-dev-workflow:spec-creation-swarm`

Next workflow: `shravan-dev-workflow:spec-review-swarm`

Terminal condition: PR-ready implementation, not merged, proving codex-router is only an account/OAuth router and otherwise pass-through, with cleaned CLI UX, correct account routing/status semantics, and current-head proof.

## User Contract

codex-router is an account router. It chooses an OAuth account, attaches the right auth, preserves account affinity where required, and otherwise lets Codex and the upstream protocol own behavior.

The router must not invent WebSocket payload policy. It must not reject, reshape, semantically validate, or gate Codex frames except for bounded metadata needed for auth/account routing and local-auth smuggling protection.

The installed CLI must feel like an installed app:

- default root is `~/.codex-router`
- normal commands do not require `--router-root`, `--state-db`, or `--secret-root`
- `account login` defaults to device auth
- test/import/proof/internal commands do not pollute normal help
- `account list`, `quota`, and session picker output are human-friendly and consistent
- quota/status renders cached data immediately, refreshes in the background or command path with a spinner, and then shows updated data
- status labels must distinguish static persisted quota prediction from live router selection

## Scope

- Remove release-path `FirstFramePolicy`, `FirstFrameTooLarge`, whole-frame first-payload JSON validation, and guardrails/tests that bless those behaviors.
- Replace whole-frame routing decisions with bounded routing/auth/affinity metadata extraction only.
- Forward WebSocket frames unchanged after metadata extraction.
- Preserve Hyper / hyper-tungstenite / tokio-tungstenite ownership of protocol mechanics.
- Fix account routing/status/schema drift, including active client/account semantics and migration state.
- Clean the CLI command contract and help surface using proper crates: clap, inquire, comfy-table, indicatif, sqlx, serde_json where appropriate.
- Add TDD tests and pyramid proof for WebSocket pass-through, account selection/routing, schema migration, CLI UX, quota/status, installed binary smoke, and three-Codex concurrent WebSocket behavior.

## Non-Goals

- Do not change Codex.
- Do not disable WebSockets.
- Do not add router-owned WebSocket fallback, retry, warmup, heartbeat, or application close behavior.
- Do not switch accounts mid-frame or create mid-message semantics.
- Do not merge without explicit user approval.
- Do not add compatibility layers for obsolete DB shapes unless a migration is the chosen hard-cutover path.

## Current Evidence

Review packet:

- `tmp/implementation-review-workflows/2026-06-25-account-router-law-review/review-packet.md`
- `tmp/implementation-review-workflows/2026-06-25-account-router-law-review/parent-findings-so-far.md`

Confirmed blockers from review lanes:

1. Release WebSocket still enforces router-owned first-frame payload policy.
   - `crates/codex-router-proxy/src/server.rs` constructs `FirstFramePolicy::new(1024 * 1024)`.
   - `crates/codex-router-proxy/src/websocket.rs` rejects non-text, oversized, and whole-frame malformed JSON before upstream open.
   - Existing tests and guardrails currently bless `FirstFrameTooLarge` instead of forbidding it.

2. First-frame metadata extraction still routes through whole-frame body semantics.
   - The WebSocket path copies the full first frame into an HTTP-style request body.
   - Account selection substring-scans for `previous_response_id`, then parses the entire body as JSON.
   - This can make ordinary Codex payload content affect routing.

3. Proof artifacts are stale against current HEAD.
   - Prior proof receipts point to older commits.
   - Some planned rows are missing, stale, or not wired into `proof-matrix.sh`.

4. `quota status` currently presents a static SQLite prediction as live router selection.
   - Live selection can account for active reservations and holds.
   - CLI status reads persisted selector inputs and builds a fresh assessment without live router state.

5. Active-load semantics are unresolved.
   - Current code releases WebSocket reservation after `response.completed`.
   - The same socket can continue to send frames after completion.
   - The spec must decide whether active load means in-flight turn or socket lifetime, then tests must prove it.

6. Live DB schema drift existed.
   - User DB had `active_client_leases` at `user_version=7`.
   - Current main migrations at `user_version=7` did not define that table.
   - Old DB was moved to `~/.codex-router/state.sqlite.backup-20260625-210712` and a fresh current-main DB was created.

## Requirements/Proof Matrix Seed

| Row | Requirement | Evidence source | Freshness guard |
| --- | --- | --- | --- |
| R1 | Release WebSocket has no router-owned first-frame payload size/shape gate. | Unit + integration test plus structural guardrail. | Must run at current post-fix HEAD. |
| R2 | WebSocket first frame is forwarded byte-identical after bounded routing/auth metadata extraction. | Real serve-path mock upstream canary with >1 MiB legal frame. | Captured upstream hash equals client frame hash at current HEAD. |
| R3 | Router only inspects bounded metadata required for auth/account routing/affinity. | Tests for nested/prompt `previous_response_id` and malformed future payloads. | Guardrail forbids whole-frame `serde_json::Value` parse in release routing path. |
| R4 | Provider/upstream frames are forwarded before any non-routing observation work. | Integration test with large/future provider frame plus slow metadata observer. | Client receives frame while observer is blocked. |
| R5 | Active-load semantics are explicit and user-visible claims match them. | Spec decision plus selector tests and live diagnostics fixture. | Fails if `quota status` calls static SQLite prediction live `next`. |
| R6 | Schema migrations are hard-cutover and current-main reproducible. | SQLite migration tests from empty and old fixture DBs. | `PRAGMA user_version` and table set match current source. |
| R7 | CLI normal help exposes only user commands and sane defaults. | assert_cmd/trycmd snapshots for help and common flows. | Installed `codex-router --help` matches cleaned contract. |
| R8 | `account login` defaults to device auth; import/proof/internal commands are hidden or removed from normal UX. | CLI parse/help tests and smoke dry-run if available. | No normal help leakage of test-only commands. |
| R9 | `quota` shows cached quota immediately, refreshes with spinner, and rerenders/refetches consistently. | Unit renderer tests + smoke fixture + live/manual proof when accounts exist. | State timestamps and refresh result shown from current DB. |
| R10 | Three concurrent installed Codex WebSocket clients route to expected safe account tags. | E2E/soak harness records redacted Authorization/account tag per client. | Current-head transcript proves selected account, not just session/model correlation. |

Rows with exact commands, test names, and sequencing must be defined by `plan-creation-swarm` after spec review.

## Stop Conditions

- Stop before code edits if the spec cannot decide active-turn versus active-socket load semantics.
- Stop before destructive DB/home/git changes unless explicitly approved.
- Stop if a failing proof gate is outside the agreed code path.
- Stop if a proposed fix changes Codex behavior rather than router account/auth behavior.

## Workflow Rules

- Current workflow owns spec creation only. Do not implement before reviewed spec and plan.
- Phase skills may recommend transitions but do not mutate official state.
- The latest orchestrator-written event in `events.jsonl` owns current/next workflow.
- Parent verifies subagent outputs before accepting findings.
- Reviewer subagents for this goal should use `gpt-5.5`; use cheaper mini models only for runtime Codex e2e target jobs, not reviewer quality gates.
