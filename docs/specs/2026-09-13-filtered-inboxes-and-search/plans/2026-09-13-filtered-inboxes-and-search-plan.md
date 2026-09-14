# Filtered inboxes and search — Implementation plan

## Planning basis and delivery boundary

- Originating planner: `plan-implementation`.
- Planning result: `ready`.
- Governing basis: `reviewed-three-artifact-design`.
- [Requirements](../2026-09-13-filtered-inboxes-and-search-requirements.md), [Specification](../2026-09-13-filtered-inboxes-and-search-specification.md), [Program Design](../2026-09-13-filtered-inboxes-and-search-program-design.md).
- Current review identity: [completed three-artifact review](../../../wip/2026-09-13-filtered-inboxes-design-review.md#completed-current-three-artifact-review). Whole-mode and scope-defense receipts, parent-verified; no required remediation.
- Planned at branch `inboxes`, HEAD `6af9ddfe39955a2cea4c43e400e92d3104a6b0ee`.
- Requested terminal: `plan-only`.
- Delivery grouping: `single:filtered-inboxes-and-search`.
- PR topology: `not-applicable` for this planning task. No PR, merge, release, or implementation permission is implied.

## Goal and scope

Let an agent request top-level messages for a project, board, or topic while independently receiving activity from watched threads. Add latest context and separate location/content search, and change the existing top-level actor/board cooldown to 60 seconds with thread guidance.

Keep scope explicit on every fetch; preserve first project-inbox use as the top-level unread start, existing watches/bookmarks and project-local summaries. Keep ordinary activity-position pagination. Search uses literal substring matching with ASCII case-insensitivity, strict scope filters and explicit archived inclusion. Thread messages in unwatched threads are searchable but do not enter the inbox automatically.

No saved defaults, watch-set digests/revisions, forced restart, page snapshots, new persistence, search index/service, daemon, compatibility adapter, or raw-history behavior rewrite. No production restart or global installation. Preserve existing proof strength and schema ownership.

## Current evidence and write boundaries

The current inbox request is project-only and unread-only; `inbox_records.rs` applies the selected project outside both eligibility branches. Raw history already has fixed-upper-bound newest/oldest keyset reads and byte-bounded pages. Board metadata has names/descriptions but no text-search operation. Cooldown is `30_000` milliseconds in the existing message transaction and already returns rounded wait/thread guidance.

Implementation owners are the existing `message-board`, `message-board-storage`, `collaboration-client`, `collaboration-protocol`, `collaboration-service`, and `agent-collaboration` packages. Matching tests and generated `.sqlx/` metadata are in scope. Public CLI guidance must reflect changed commands. Root schema migrations, Host lifecycle, unrelated automation and global settings are outside scope.

Relevant existing surfaces:

- `crates/message-board/src/board_operations/{inbox_operations,message_operations,setup_operations}.rs`, `board_messages.rs`, `board_failures.rs`.
- `crates/message-board-storage/src/{inbox_records,message_history_reads,message_write_operations,message_row_decoding,storage_support,board_topic_records,project_records}.rs`.
- `crates/agent-collaboration/src/board_commands/`, client `board_operations.rs`, service `board_request_dispatch.rs` and validation, protocol `control_schema_document.rs`.
- Domain tests; storage `inbox_history_and_summary`, `cross_project_behavior`, `activity_boundary_validation`, `concurrent_inbox_latency`, and migration tests; service `board_control_path`; CLI `board_cli`, `board_write_uncertainty`, `board_debug_acceptance` and `board_live_support`.

Split growing files only along the named responsibilities (inbox reads, search reads, argument preparation). Current inbox storage is 455 lines, CLI arguments 533, preparation 472. Do not introduce a generic repository/query framework to avoid file growth.

## Proof-bearing slices

Execute serially because contracts, fixtures, dispatch and generated metadata overlap. Each slice includes its typed public path and proof; no standalone contract/refactoring phase with no consumer.

### 1. Filtered unread and latest inbox

Cover F1–F4/F7/F9, C1–C5/C9, V1–V3/V7.

Extend the typed inbox request/page for explicit project/board/topic scope and unread/latest mode. Update domain, SDK, schema, dispatch, CLI and all existing request constructors together. Resolve scope ancestry before initialization. Constrain only the top-level branch; use independent active-watch eligibility across the selected service. Keep each item's actual ancestry, including cross-project state events. Preserve per-project summary meaning and validate all contributing stored boundaries.

Unread uses existing start/bookmark/watch boundaries and first-use mutation handling. Latest reads historical messages without initialization or acknowledgement. Use the existing cursor key and captured-upper/last-position pattern; old incompatible inbox cursors fail explicitly. Watch changes do not force a latest restart. Keep raw message history unchanged.

Before changing behavior, add permanent domain/store scenarios expressing the missing filter/mode behavior and observe the expected rejection or incorrect selection; compile errors alone are not red behavior proof. Then prove:

- Two projects, multiple boards/topics, matching and unrelated top-level messages, an unwatched thread inside scope, and a watched thread outside scope.
- Correct location and acknowledgement scope for messages and resolve/unresolve events; references do not expand eligibility.
- First-use history exclusion/current start, repeated and alternate-scope fetches, pre-existing watches, self exclusion, restart persistence, and cross-project acknowledgement.
- Latest includes historical/self/already-read messages without state initialization. Missing scope fails on fresh/continuation requests.
- Item and byte limits, stable-eligibility completeness, fixed upper bound, ack/continue, watch changes with ordinary continuation and fresh-fetch visibility, invalid/mismatched/foreign cursors.

Earliest integration gate: extend the real socket-pair Control test and CLI parsing/JSON tests before considering this slice complete. The SDK's unread uncertainty handling must not be accidentally removed while latest becomes nonmutating.

### 2. Separate discovery and message search

Cover F5/F6, C6/C7, V4/V5.

Add separate typed operations through the same domain/SDK/schema/dispatch/CLI path. Discovery matches names/descriptions and returns kind, stable identity and ancestors. Message search matches body text with explicit location/message-kind restrictions, including inside a specified unwatched thread. Reuse storage-owned validation and decoding; bind literal text as data. Preserve stable discovery ordering and newest-first activity ordering for messages.

Start with permanent public/domain and real-store scenarios that fail because search is missing. Prove ASCII case-insensitive substrings, exact non-ASCII behavior, literal punctuation including `%`/`_`, query bounds, empty/no-match results, strict ancestors and message kinds, reference-target-only nonmatches, real ancestry, and unchanged personal reader state. Exclude archived boards unless requested, including explicit archived thread targets. Archive-between-pages removes remaining hits without cursor invalidation; changing query/filter inputs rejects continuation.

Extend Control/CLI roundtrips at this slice, not only at final validation. Measure broad no-match and many-match workloads under the existing serialized store; do not claim query latency from bounded output alone. Preserve existing concurrency-test thresholds. If direct queries cannot meet existing service budgets, return evidence to Program Design instead of adding indexes, pools, or background search machinery silently.

### 3. Sixty-second cooldown and agent guidance

Cover F8, C8, V6.

At the existing actor/board transaction owner, change the interval to `60_000` milliseconds and make the existing structured failure clearly direct the agent toward an existing unresolved thread. Retain `topLevelMessageCooldown`, rounded `retryAfterSeconds`, and `postThreadMessage`. No timestamp migration or reset.

First demonstrate the current 30-second behavior fails the new 60-second scenario. Permanent tests must prove 59,999 milliseconds rejected, exactly 60,000 allowed, cross-topic sharing within a board, independent actors/boards, unchanged acting-for semantics, exempt thread posts, and no message/watch/cooldown effects on rejection. Use existing time-calculation and real-store fixtures; no sleep-based minute-long unit test or new production clock abstraction merely for this change.

CLI proof must inspect the actual rejected response and actionable guidance. Reconcile public guidance for this feature and the duration. If a runtime skill reference must change, use its owning skills-creation workflow for that bounded command/guidance edit; do not redesign the skill.

### 4. Integrated proof and quality

Join slices 1–3 through the existing isolated debug Host/CLI path. Extend the established `board_live_support` journey to discover a topic, retrieve filtered latest/unread, preserve outside-scope watched-thread traffic, search inside a thread, acknowledge processed activity, and observe cooldown guidance. Retain the existing two-Luna and restart-persistence admission/proof gates instead of claiming mocked tests are live proof.

The real Host example is `crates/codex-router-host/examples/automation-debug-host.rs`. Its explicit help supports `--run-directory /tmp/NEW-DIRECTORY --router-binary /ABSOLUTE/target/debug/codex-router [--port 18787]`. It owns its isolated children. Verify exact help, private directory, debug profile, nonproduction port and process identity at execution time. Do not operate an existing production process. Restart proof uses only the explicitly owned debug Host under the harness's restart boundary.

Runtime acceptance uses `CODEX_AUTOMATION_PROOF_ROOT` pointing to the owned run directory; `board_debug_acceptance` is intentionally ignored by ordinary suites. Run its two-agent journey explicitly, then its separate persistence test after the harness-authorized owned restart. Record commands, outputs, counts/exit status, and observed state. Missing debug access/admission is a reported blocker, never a reason to weaken the gate.

## Commands and validation

Run from repository root. These are planned commands, not executed evidence:

```sh
cargo test -p message-board -p message-board-storage
cargo test -p collaboration-protocol -p collaboration-client -p collaboration-service -p agent-collaboration
scripts/tooling/bootstrap-tools.sh sqlx
python3 scripts/tooling/prepare-sqlx.py
python3 scripts/tooling/prepare-sqlx.py --check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --profile ci --workspace
```

Regenerate checked SQL metadata only when the SQL changes, then verify it against native migrations. The script already covers both account and message-board databases; do not introduce a combined schema or write live user state.

Preserve required `.github/workflows/ci.yml` gates, including tool-bootstrap tests, build variants, dependency policy/audit and existing workspace tests. Focused tests guide development; full gates close the integrated work. Read CI's exact commands at execution time rather than duplicating or weakening them. Check `du -sh target/debug` after extensive builds; cleanup requires the existing no-active-dependents check and must not stop processes.

Explicit live commands after the existing harness's environment admission:

```sh
cargo test -p agent-collaboration --test board_debug_acceptance two_luna_agents_exchange_a_verified_finding_through_the_board_cli -- --ignored --nocapture
cargo test -p agent-collaboration --test board_debug_acceptance board_state_survives_owned_debug_host_restart -- --ignored --nocapture
```

## Dependencies, completion, and stops

```text
1 inbox vertical path -> 2 search vertical path -> 3 cooldown/guidance
                     -> 4 integrated real-path and full quality proof
```

The serialization is for shared edits/generated artifacts; no distributed worktree or separate PR topology is needed. Every F1–F9 requirement has a slice and Specification proof ID above. Existing posting, archive, references, identity, summary, migration and raw-history behavior require regression preservation, not redesign.

Stop for a discrepancy in scope/ownership, missing public behavior, failed invariant, or evidence that the chosen simple query cannot fit existing service budgets. Diagnose validation failures before bounded repairs at the current owner. Do not add state, weaken proof, silently coerce corruption, or change the accepted pagination semantics to make tests pass.

Implementation completion would require all scoped behavior and quality tests, current CLI/runtime proof, checked metadata, and current guidance. This document's terminal is plan-only: producing it does not execute those changes or authorize implementation, publication, version tagging, merging, or production replacement.
