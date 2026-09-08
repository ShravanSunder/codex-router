# Scheduled workflows implementation plan

## Canonical planning record

- Originating planner: plan-implementation.
- Planning result: ready.
- Governing basis: reviewed-three-artifact-design.
- Requirements: [Requirements](../2026-09-07-scheduled-agent-workflows-requirements.md).
- Specification: [Specification](../2026-09-07-scheduled-agent-workflows-specification.md).
- Program Design: [Program Design](../2026-09-07-scheduled-agent-workflows-program-design.md).
- Review/remediation: [Astra review](../../../wip/2026-09-07-automation-astra-review.md) and [parent verification](../../../wip/2026-09-07-automation-review-resolution.md).
- Requested terminal: plan-only. This plan does not authorize implementation, service launches, rebase, merge or release.
- Delivery grouping: single:local-scheduled-agent-workflows.
- PR topology: one-pr for later implementation, following merged PR1. Event wake-ups and message board are separate future work.
- Planned at: feat/agent-messaging-v2, HEAD 70c5393f0b141c7603efd966358061020249de2f.
- Tracking: no external tracker; no tracker mutation in planning.

## Goal and boundaries

Implement local scheduling, durable delivery and time-based wake-ups through the shared Rust SDK and agent-sessions CLI. Preserve the nine-table model, UUIDv7 thread bindings, ordinary human/agent communication, one active scheduled workflow, first-fire semantics and two-month event history. Keep execution timeout at a configurable one hour and Luna summary timeout at configurable fifteen minutes.

No automatic agent replies, wake-on-event, no-reply fallback, shared board, remote federation, native relocation, custom DST engine, general workflow engine, upstream Codex changes or production replacement. No additional tables without returning to design. Instruction revisions remain; schedule/wake history uses retained events. Runtime proof invokes only Luna and isolated debug paths.

## Current integration evidence

The current workspace has communication protocol/client/service crates, native generation/schema admission, Host composition and CLI message/observation paths. It has no agent-automation or automation-storage crate. Native message submission intentionally auto-steers active work; scheduled readiness must use the separately designed eligibility path. Ordinary Control calls time out after thirty seconds; first-fire waiting needs a dedicated observation connection. Host configuration is immutable launch configuration; the mutable automation settings adapter is an explicit addition.

Current relevant sources: message_commands.rs and event_observation_commands.rs in agent-sessions; control_connection.rs in communication-client/service; native_message_dispatch.rs and message_effect_state.rs in communication-service; host_configuration.rs and communication_runtime.rs in codex-router-host; native_payload_schemas.rs and native_thread_operations.rs in codex-native-integration. CI uses cargo fmt, Clippy, nextest, deny and audit. scripts/proof-matrix.sh is an older workflow-specific harness, not automatic proof of this feature.

## Entry checks for a later executor

Read this plan, all three design artifacts and review resolution fully. Inspect git status and current AGENTS instructions. Preserve all concurrent changes. Verify the design artifacts remain applicable; any change to the nine-table or messaging model returns to design.

Inspect the merged PR1 tree against this older branch before source changes. Do not silently rebase: history integration requires an explicit directive. If integrating the merged base is necessary, identify the exact difference and obtain that directive before changing history; then revalidate affected integration paths. Planning readiness is not permission to run this plan against a changed base without checking it.

Confirm installed debug Codex capabilities from its generated schema, not from upstream source alone. No production startup experiment is permitted. If debug provider authentication or availability blocks real proof, report the exact blocker and continue only independent local development checks; do not substitute a production endpoint.

## Slice graph

```text
A. Domain/storage + generated contracts
                 │
                 ▼
B. Timed wake-up through CLI/SDK and Host
                 │
                 ▼
C. Schedule/run execution + continuity
                 │
                 ▼
D. Import/configuration/recovery/history integration
                 │
                 ▼
E. Debug Luna proof + aggregate checks + implementation review
```

A is a contract/storage slice whose immediate consumer is B, then C. Shared wire, service and schema-generation write sets make these sequential milestones in one PR. Do not run simultaneous Rust workspace builds. Delegate long test/build/watch procedures only to Luna Operators; independent code review is separate from runtime tests.

## A. Domain, nine-table storage and wire contracts

Write surfaces: new crates/agent-automation and crates/automation-storage, workspace manifests/lock, communication-protocol automation contract modules and permanent tests. Use responsibility-based two-to-three-word filenames and existing lint inheritance.

Start with failing behavior tests for typed IDs/variants, instruction revisions, conditional run claims and stale release, ownership FKs, operation replay conflicts, event retention preserving current state, imported continuity after reload, and current-versus-retained attempt history. Use real SQLx/SQLite for transactional behavior; use injected clocks for timing. Do not call generated DTO validation runtime proof.

Implement the nine tables and typed repository transactions exactly as designed. Generate public schemas from authoritative Rust wire types. Verify all method/error variants have matching closed schemas and that imports fit complete request/response frames. Characterize current message paths before extracting a reusable native submission boundary.

Gate: storage tests reject cross-schedule pointers and duplicate bindings; no SQL triggers/status enums; partial effects survive serialization; generated schema is consumed by B rather than left unintegrated.

## B. Timed wake-up vertical path

Write surfaces: agent-automation timing/wake/delivery modules, communication-service dispatch, communication-client wait/inspection, codex-router-host composition, agent-sessions wake commands and permanent protocol tests.

Add failing CLI/RPC tests for wake send/show/list/pause/resume/cancel, command deduplication after lost response, first-fire waiting distinct from acceptance, pause/cancel errors, exhausted one-shot, and original expiry after resume. Use Croner 4.0.0 five-field parsing and explicit timezone; no custom DST modes.

Wire actual Host → storage → timing → native adapter. First-fire subscription uses its dedicated connection; test a concurrent ordinary command remains usable. Exercise generation changes, rejected queue/steer preconditions, pause versus dispatch, and uncertain native outcome with no automatic resend. Capture current attempt effects before I/O and archive old attempts transactionally.

Gate: a real CLI subprocess creates and inspects a wake through the service and SQLite. Simulated native fixtures may prove failure classification, but must be labelled integration fixtures until E establishes actual model delivery.

## C. Scheduled execution and summary

Write surfaces: schedule/run domain and repository methods, native typed start/fork/history helpers, service/SDK/CLI schedule and run methods, permanent execution tests.

Add failing tests for one active run including required summary, coalesced waiting work, disable preserving created runs, current-input capture, immutable binding on rebind, busy-target no-submit, and guarded release. Test two independently opened SQLite connections racing admission, not only sequential updates.

Implement exact native readiness/read/resume/start receipt flow. Known busy preparation consumes no execution budget. Start races retain actual native evidence; normal messages remain allowed. Observe only the recorded turn; uncertain effects retain exclusion.

Implement fresh-thread continuity and imported summary capture, separate read-only Luna summarizer, fifteen-minute captured summary timeout, inspect/retry/skip, and summary content lookup by run ID. No native subagent tools or automatic B-to-A completion messages.

Gate: repository/RPC integration proves execution/summary state distinctions, timeout request versus cessation, captured inputs surviving edits, and uncertainty blocking successors. Native capability mismatch is a design/evidence blocker, not permission for an upstream patch.

## D. Portability, configuration and crash recovery

Write surfaces: existing automation modules, Host-composed AutomationConfigurationStore, CLI import/export/configuration/history surfaces and permanent filesystem/process tests.

Add failing tests for JSONL round trip, truncation/frame limits, stable schedule ID, overwrite/new change ID, shared instruction collision and disabled import while native endpoint is unavailable. Verify imported summary survives reload and event pruning without a synthetic local run.

Exercise file-backed settings update at admitted receipt, temp write, rename, directory sync and final commit boundaries. Startup must reconcile settings with operation receipts before admitting new automation. Active runs keep prior effective budgets. Use a real temporary filesystem fixture inside permanent tests.

Crash the owned test service around native dispatch-intent/receipt and local creation commits. Reconnect must return existing result or uncertainty; never blindly repeat fork/start. Test event cleanup with a controlled date and actual SQLite deletes; initial history pages disclose coverage and latest state remains available. Inspect all structured nextAction values against real SDK/CLI operations.

Gate: no recovery scenario depends on a retained event older than two months to reconstruct live state; no production database/profile is written by fixture tests.

## E. Debug-only Luna acceptance and delivery boundary

Build once, reuse artifacts across acceptance scenarios. Before launch, verify router-root/debug profile, socket ownership and ports; record only nonsecret identity/port metadata. Use normal Codex home with codex-router-debug profile and dedicated absolute CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET parent. Distinct socket alone does not isolate upstream startup serialization in Codex home: do not stop a production app-server to acquire the lock. If the real debug setup cannot coexist safely, report the isolation blocker rather than redirecting CODEX_HOME or touching production.

Use a permanent acceptance runner under scripts/proof-tools or the established crate integration harness. It must reject a missing debug profile, production endpoint selection and any model other than gpt-5.6-luna before submitting work. Capture its --help/command contract in the implementation; no guessed startup command is authorized by this plan.

Required real scenarios:

1. Luna A invokes the actual CLI from its sandbox, arranges a timed message to Luna B, and B explicitly replies through message send. Prove accepted input and observed content separately; no event-generated reply.
2. First-fire wait returns when firing is recorded even if native delivery is still unavailable; later acceptance appears under the same delivery ID. Pause before first firing and skipped one-shot produce typed errors.
3. Queue on unloaded thread rejects; explicit steer without an active turn rejects; auto delivery reaches idle and busy threads with truthful receipts.
4. A scheduled run waits for externally busy work, then executes; ordinary message input remains allowed. Two scheduled runs never overlap, including required summary.
5. A fresh-thread run uses the previous Luna summary; summary failure/timeout preserves worker outcome and exclusion until safely resolved.
6. Restart only the owned debug service/processes: durable scheduled work and first-fire state remain, uncertain native effects are not replayed. Match all target process IDs before stopping any test-owned process.
7. Export/import with preserved identity and disabled activation, inspect history and demonstrate two-month event cleanup without deleting live state.

Keep delayed examples short in live proof; use injected clocks for hour/month boundaries. No need to wait fifteen minutes or two months to prove numeric deadline/retention logic. A live unavailable/auth failure is a blocked gate, not a pass or a reason to use production.

## Obligation/proof coverage

| Requirements | Owning slice and proof |
| --- | --- |
| S1, S12, S13, S14, S34, S35, S42 | A/D: package boundaries, commented nine-table schema, real SQLx, typed wire validation |
| S2, S3, S23 | A/C: instruction history, current-input capture and immutable run evidence |
| S4, S20, S21, S22 | D/E: portable identity, default discovery, disabled import and local preparation |
| S5, S6, S7, S8, S15, S16 | C/E: disable/catch-up, admission, binding and continuity |
| S9, S10, S17, S19, S38 | C/D/E: worker/summary budgets, native cessation and current summary inspection |
| S11, S18, S28, S31 | A/B/D/E: retained data, CLI/SDK feedback and no uncertain replay |
| S24, S25, S26, S29, S32, S33, S36, S37, S39 | B/E: timed reminder and first-fire lifecycle |
| S40 | D: automatic event retention and historyExpired |
| S41 | C/E: ordinary input allowed without overlapping scheduled runs |
| S27, S30 | Explicitly deferred; verify no automatic completion/event wake was introduced |

## Commands and final gates

Run from workspace root. During slices use focused package commands, for example `cargo nextest run -p agent-automation -p automation-storage`, then include each touched integration package. If nextest is unavailable, report/install under the environment policy rather than silently dropping tests. New crates must exist before their command is meaningful.

Final required Rust/CI-aligned checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
cargo audit
python3 -m unittest scripts.tests.test_update_homebrew_formula -v
```

Preserve current CI's feature/install/PTY gates; inspect the current workflow before final validation rather than treating this list as permission to omit other mandatory jobs. Run relevant non-Rust lint/type checks if adding harness code in those languages. Record commands, counts, skips and exit codes. Broad tests do not replace real debug acceptance. An unrelated CI/environment failure is reported with scoped results; no unrelated remediation without authorization.

A later implementation-ready handoff must include current source/diff and evidence for independent implementation review. Do not call design-review coverage implementation approval. PR/release work follows its owning workflow; version bump before merging user-facing code is required, but this plan does not authorize merge/tag or replacing production processes.

## Stop/replan conditions

Stop code edits and return to design if native capabilities cannot satisfy the chosen contracts, the nine-table model requires an unplanned owner/table, lost effects cannot be represented, or a proof gate would need weakening. Resolve only test-mechanics failures inside the agreed scope. Preserve code on conflicting concurrent edits. Do not make up evidence for a blocked debug environment. No milestone, green unit suite or subagent completion is itself completion of the eventual implementation.
