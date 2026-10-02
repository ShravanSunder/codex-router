# Release Notes

## 0.1.59

- **Breaking:** remove CLI `board thread listen` and MCP `board_thread_listen` and its control tools; use per-thread subscriptions and poll-mode `board thread wait` instead.
- Persist thread and topic subscription policies, lifecycle, per-root batching windows, and backfill eligible existing participants. Migration `202609170001` was rewritten in place: local or debug board databases that applied the earlier branch version fail with `InvalidSchema` and may need to be deleted; production databases are unaffected.
- Subscribe participants when they join a watched thread, batch thread activity, and hold thread notices or DMs for targets that are not running. Store Router pushes as 30-day `router://` records and fetch them with `agent-collaboration show`.
- Print message-send failures as one `error: <explanation> — <next step>` line without `--json` and held DMs as `held: <link> — delivered when <target> is next running`. Delivered or held outcomes exit 0, rejected deliveries exit 4, and unknown outcomes exit 5. Unknown DM outcomes point to `show <link>` before retrying. Ambiguous Claude terminal errors list live claimant PIDs and short names; structured details retain full names and cwd where available, and the line says to close one terminal or run `/branch`.
- Remove `message reply --expect-sender` and implicit reply-to-latest-sender behavior; a DM reply now requires its push id or `router://` link.
- Reject `steer` and generation-guarded DMs when the target is not running; these sends are not held for later delivery.
- If a wait response is lost after activity may have been handed off, report an unknown outcome and inspect `board thread subscriptions` and `board inbox fetch` before waiting again.
- Publish the generic `overloaded` admission error for message send/reply/inbox/history and `router/show`, matching rejection before route dispatch.
- Watching or subscribing to a topic with no board activity yet now works and covers the first later thread; existing watch rows are preserved.
- Retry known-rejected Provider ACP scheduled-run submissions with the same stored push id, leaving the run available for another attempt.

## 0.1.58

- Provider setting rejection errors now name the setting, requested value, and advertised choices; display truncation is marked with an ellipsis.
- Hide empty sessions from the picker by default.
- Encrypt pooled account credentials with a Router-owned Keychain key. After upgrading, authorize Router once at the first Host restart; refreshes in that running Host do not prompt. The Host migrates existing pooled credential files to encrypted envelopes, reads each credential back for verification, then removes the plaintext file. If migration stops early, the Host still starts, Claude requests report the incomplete migration, Codex requests keep the existing credential-unavailable response, and `account list` and `quota` identify accounts still to convert; the next Host restart resumes migration without marking accounts as needing login.
- Add `serve --claude-five-hour-reserve-percent` to configure when Claude's five-hour window enters the reserve tier; defaults to 95 percent and accepts values from 1 to 99.
- Route Claude accounts by their five-hour and weekly quota windows. Claude accounts are not yet shown by `quota`; that view is added in PR4.
- Add provider identity to accounts and session pins, and migrate existing rows to OpenAI. The state migration blocks downgrade because older binaries reject its unknown migration version. Restart `serve` after upgrading; an older running process still writes pins with the previous schema and those upserts fail.
- Let `account login` create a new credential generation for the same account and provider; refuse a label owned by a different account or provider.
- Set Codex session-pin idle expiry to 75 minutes by default, configurable with `serve --session-pin-idle-ttl-seconds`; show each account's provider in `account list`.
- Restore CLI and MCP prompts to existing Codex conversations when Claude or Cursor ACP routes share the Host. The client now sends the exact SessionRef on `session/load`, so a new ACP connection selects the Codex route without weakening rejection of unknown bare Session IDs.
- Invalid `conversation create --from` identity JSON now exits 2 with `invalidField`; provider creates accept Human creators and Approvers, including an owner-selected `--approver-owner` shortcut.
- `wake send --wait-until-first-fire --json` now emits one result with `result.record.firstFire`, or one error retaining the created wake under `created`.
- Scheduled runs can deliver their first input to a Codex conversation created without an initial prompt.
- Scheduled runs to materialized existing Codex conversations now compare the native workspace with the schedule's declared workspace and deliver the input.
- Name a Codex thread held by another client's active writer as a typed message rejection, with guidance to send from the holding Codex client instead of retrying the same Router path.
- Isolated Hosts now work in release builds and accept `--require-debug-isolation` with home-default mode; a forged `HOME` cannot reach launchctl and must satisfy isolated debug-profile and socket checks.
- Advertise an additive MCP tool output union that validates both unchanged successful structured receipts and typed structured errors. Reconnect existing MCP clients after a Host upgrade to refresh cached tool schemas.
- Enable configured Claude and Cursor ACP providers by default from owner-editable `providers.json`. A failed provider reports endpoint-specific reason and fix without taking down the other endpoints.
- Use one CLI and MCP conversation surface for Codex, Claude, and Cursor, including create, prompt, load, and operation inspection. Claude and Cursor can cancel one exact operation; Codex directs callers to turn interrupt. Provider operations retain caller IDs and report completed or pending work.
- Route messages, wakes, listen pushes, approvals, and scheduled runs through the selected Codex, provider ACP, or live Claude Code peer route. Delivery receipts expose the observed outcome and reachability; `peerMessageWritten` confirms a socket write, not a peer reply.
- Preserve Codex create-then-message across frontend closure while the Host remains running. Provider schedules run on existing sessions and finish from provider settlement without a summary.

## 0.1.57 - 2026-10-01

- Add Claude account login with `codex-router account login --provider claude` and show five-hour and weekly usage in `codex-router quota status`.
- Route new and resumed Claude Code sessions through Router's pooled accounts with `agent-sessions --provider claude`. Claude no-account responses explain unavailable credentials, accounts that need login, usage limits with reset hints, and accounts held by quota floors or stale weekly observations.
- Bound OpenAI device login with a 5-second minimum poll interval, a 15-minute overall limit, and 30-second HTTP request timeouts.
- Assess every account against one clock instant per selection to prevent account-order drift at second boundaries.

## 0.1.54 - 2026-09-30

- Add target presence reporting and loaded-only delivery policy as internal groundwork for thread subscriptions.

## 0.1.29 - 2026-09-17

- Rename the human session picker binary from `agent-session` to `agent-sessions`; it replaces the retired standalone `agent-sessions` install and keeps every flag, including model and effort restoration on resume and fork.

## 0.1.28 - 2026-09-17

- Require explicit model, effort, and access choices for new and forked Codex sessions; preserve those choices through resume and report source-labeled native settings evidence.
- Add `write-restricted` and `workspace-write` access with owner-private shared scratch, explicit project work areas, inherited approval behavior, and supported exact Control-socket configuration without broad Unix-socket grants.
- Add persisted session rename and explicitly scoped session discovery by cwd, checkout, repository, source, and query, with explicit names separated from derived titles.
- Standardize finite collaboration CLI JSON results on `result.page.records` for lists, `result.record` for reads, and `result.record` plus `result.effects` for mutations. Remove the former top-level result fields `sessions`, `message`, and `thread`; every result now identifies its CLI and service versions.
- Accept either `--to <SessionRef JSON>` or `--endpoint` plus `--session` across commands that target a session, resolve `self` consistently on board actor and reader flags, reject forbidden C0 bytes at text write boundaries, and expose reasoned native rejections.
- Add repository-scoped board thread discovery across every project attached to an explicit repository path.
- Add the unique `implementer` Thread seat and report both Orchestrator and Implementer holders on Thread and Participant reads.
- Add Topic Watch and Topic Listen selection, including roots created after arming. Thread Listens now use fixed `short` and `long` lifetimes, a five-minute debounce with a twenty-minute cap, terminal finalization records, and Codex-only `--deliver session` background delivery with silent-mark heartbeats.
- Supply the Router Control socket's network profile to the managed app-server itself, so Router-launched threads reach the socket with every public host allowed, exactly one Unix socket permitted, and local binding off.
- Make `--model` and `--effort` optional on `--fork`, defaulting to the source thread's persisted values, and `--effort` optional on `--session`. A resume that asks for a different effort is allowed and reports `effortChange`, because the provider's prompt cache for that session is not reused.
- Show each session's model and reasoning effort in the `agent-sessions` picker and restore them when resuming a picked session.
- Match the stored `--repo` scope to the human catalog: a row that names an origin is decided by that origin alone, so another repository's row can no longer be selected by a matching directory name.
- Return the entity itself as `result.record` for single reads of a message, thread, or listen. Choosing each command's envelope through explicit constructors is deferred to a follow-up.
- Add a skill-to-CLI contract test that replays every documented invocation in the canonical collaboration skill against the built CLI.

## 0.1.27 - 2026-09-15

- Add explicit Thread Participants with closed Roles (`orchestrator`, `advisor`, `reviewer`, `participant`), one open Orchestrator, sequence-based presence, and Participant listing on Thread reads.
- Add `board thread create`, `join`, `leave`, and `participant list`, with explicit Watch and Listen choices, `--actor self` for Codex and Claude Code sessions, and corrective `nextAction` details on refusals.
- Require session Participants for Thread replies and Listens and the session Orchestrator for resolution. Humans remain able to post, Listen, and resolve without joining; human resolution closes every open Participant.
- Stop message posting from changing Watch state. Session topic-placement posts now direct callers to `thread create`; human topic-placement posts remain available without creating a Participant.

## 0.1.26 - 2026-09-14

- Add process-owned `board thread listen` for watched or named Threads, with Once and Repeating modes, durable Delivered positions, explicit `--acknowledge` or `--no-acknowledge`, and JSON Batch sets on stdout. Once Listens require `--max-wait <duration>`.
- Use the Control protocol's long-poll `ThreadWait` fallback because Control 1.0 has one response per request; the CLI loops that request for Repeating Listens.
- Coalesce Activity with the fixed `THREAD_LISTEN_DEBOUNCE = 30s` window and `THREAD_LISTEN_DEBOUNCE_CAP = 120s` cap.
- Restore `agent-sessions` as the singular standalone executable name for the session picker while `agent-collaboration` remains the collaboration and board CLI.

## 0.1.2

- `codex-router sessions --new` starts a fresh Codex session through the router profile.
- `codex-router sessions` always offers a `New Codex session` picker choice.
- Session launches pass trailing Codex flags through, including `--yolo`, for both new and resumed sessions.
- Legacy router-owned `sessions --scope` remains rejected; use `--checkout`, `--repo`, or `--any`.
