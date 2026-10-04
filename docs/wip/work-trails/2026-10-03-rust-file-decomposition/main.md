# Rust file decomposition work trail — unshared

This repository-local trail preserves the existing task. No repository-associated shared board home has been established. Detailed local coordination history was archived privately in ignored scratch before preparing this public-safe checkpoint; no history or owned work was discarded.

## Scope and delivery boundary

Owner-requested work: behavior-preserving, responsibility-based decomposition of Rust files over 1000 physical lines, plus an error gate in the existing lint pipeline. Keep owning crates, public API/wire/storage/security behavior, tracing targets, literals and every proof gate intact. Requested whole-work terminal: PR ready and unmerged. No release or production process replacement.

[Proposal](../../2026-10-03-rust-file-decomposition-proposal.md) is blocked candidate context, not an executable whole plan. [Board citation map](../../2026-10-03-board-source-citation-map.md) retains eight unchanged source/symbol citation homes.

## 2026-10-03: inventory and independent tooling

Source baseline `7a8cbd8943e6fb2dac23bcde7069203925003918`: 1640 tracked Rust files, 28 oversized, 11 crates. Read-only evidence collection mapped their responsibilities and decisive fixture/guard/module dependencies. No generated/vendor exceptions or temporary stand-ins were admitted.

Exact source reorganization was cleared by the host and fixes tracks; selector non-overlap was confirmed, and board docs-only disposition was reported through coordination. Per-owner future maps remain required before overlapping feature writers re-anchor.

Under an explicit independent-slice instruction, added [checker](../../../../scripts/tooling/check-rust-file-size.py) and [permanent tests](../../../../scripts/tests/test_rust_file_size.py). Historical red proof failed for the missing checker; green proof passed all 16 real Git/CLI tests. Full-repo CLI exited 1 with the expected 28 exact path/count pairs. Python syntax/indentation checks passed. CI and Rust sources were unchanged.

Historical environment observations: pinned Rust/Cargo versions and formatting passed. A narrow offline check failed before compilation because `chrono` was unavailable; ordinary online registry checks stalled and were interrupted. These observations do not prove current availability or firewall causation. Rust compilation, tests, Clippy, SQLx/runtime proof, independent review and PR readiness were not established.

## 2026-10-04: relocation and local checkpoint authorization

Verified relocated worktree, repaired Git common directory, original branch/source baseline and the exact prior untracked work. Original session, model and effort were retained. No helper restart, feature work, Rust/dependency attempt or production action followed relocation. Historical private machine/session references remain outside committed docs.

The owner then authorized coherent local commits of this task's existing work only: no pushes, merge, additional implementation or security change. If signing fails, stop and report; no unsigned fallback is authorized for these checkpoints.

Decision: checkpoint the tested checker independently from sanitized planning/trail/citation documents, using exact file staging. Preserve original detailed documents privately in ignored scratch and retain their scope, decisions, historical evidence and current uncertainty in these durable views. This makes the public-bound checkpoint discoverable without exporting private identifiers.

Post-relocation validation: `python3 -m unittest scripts.tests.test_rust_file_size -v` passed 16/16, exit 0, 1.628 seconds. `python3 -m py_compile scripts/tooling/check-rust-file-size.py scripts/tests/test_rust_file_size.py` and matching `python3 -m tabnanny` commands exited 0. Actual checker invocation exited 1 with the same 28 oversized sources; expected baseline enforcement, not compliance. Document/source links and privacy scans are checked before staging. No Rust checks were rerun.

## Current outcome and next owner action

Independent checker slice is implemented and tested; the whole refactor is incomplete. Canonical whole-plan identity: none. Rust source extraction, final CI integration, required Rust proof/review and PR delivery remain held. No open stand-in.

Local checkpoint commit results are reported through the task's existing coordination route. Signed commit failure is a checkpoint blocker; it must not be worked around by changing signing settings.

Current dependency availability is unverified. Ownership clearances remain known; they do not lift the feature-plan hold. Owner action needed: lift the relevant hold and authorize the next bounded baseline check when ready. After that, verify actual dependency/compile feasibility, settle exact cuts and proof maps, and create an admissible executable plan before any Rust writes.

## Local checkpoint result and environment report

Checker checkpoint `958bf6c5` contains only the existing checker and its permanent tests. The commit used the configured SSH signer; no signing setting was changed and no unsigned fallback was used. Documentation is checkpointed separately after public-metadata/link validation. No push or merge.

A coordination report now attributes the earlier registry issue to sandbox permissions and says dependency fetching passed elsewhere after a policy refresh. That is reported external evidence, not a current build/test result from this worktree; the earlier slow-transfer receipt alone did not establish that cause. The current feature-plan hold still applies. No configuration, shell, permissions, runtime or dependency change is made under this informational report. No self-wake or extra build is authorized.

Pending work is intentional owner hold plus missing worktree-specific compile/proof and executable-plan admission, rather than an asserted current firewall failure. Next action remains a narrowly authorized baseline check after the owner releases that hold and the execution environment is refreshed through its supported coordinator path.

Signature evidence: the checker commit object contains an SSH signature. `git verify-commit HEAD` exits 1 because local SSH signature verification requires a configured, existing allowed-signers file. No trust configuration was changed. Successful signed commit creation and unavailable local signer-trust verification are reported separately.

## Updated overlapping owner reservation

The fixes track has resumed its separately authorized native-interrupt work and temporarily reserves `crates/collaboration-mcp/src/mcp_server/tests.rs` for real propagation tests. Defer decomposition of that exact test owner until its per-owner handoff supplies current source/test/filter evidence. Other previously cleared oversized-file surfaces remain cleared; this reservation and the decomposition feature-plan hold are distinct. No source edit or build follows this notification.

## Checkpoint reporting outcome

Created local SSH-signed checkpoints `958bf6c5` (existing checker/tests) and `0d1c47f5` (sanitized proposal/citation map/trail). Both include only this task's reviewed files. Post-checkpoint Git state is clean; no push, merge, unsigned fallback or extra implementation occurred.

The requested supported Router reply was rejected as not submitted: requester thread was not found or never started, with a non-retryable receipt. No duplicate thread, identity change, resend, wake, production restart or alternate communication route was attempted. The final user-facing report carries the substantive status until the original requester's continuity is restored. Exact local receipt context remains outside public docs.

Remaining continuation holds: owner release of this task's feature-plan hold, worktree-specific baseline/compile proof and whole executable-plan admission; the active MCP test-owner reservation requires its per-owner handoff. Reported environment recovery elsewhere is not proof here. Local SSH signature trust verification remains unavailable because the allowed-signers file is not configured; signatures are present and no security settings were changed.

## Baseline and exact cut planning — 2026-10-04

The owner-authorized bounded baseline completed after the refreshed environment: `env -u CC -u CXX -u LDFLAGS -u CPPFLAGS PATH=/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin RUSTUP_AUTO_INSTALL=0 cargo check -p codex-router-state --locked` exited0 and finished the dev profile in0.37s. This is current dependency/compiler readiness evidence for the package only; no Rust source change or full workspace proof.

Fixes Lead released the temporary reservation for `crates/collaboration-mcp/src/mcp_server/tests.rs`; it remains untouched. All28 pure-reorganization clearances remain.

Authored exact first-cut map at `docs/wip/2026-10-04-maintenance-actor-cut-map.md`. Proposed write set is only the existing `crates/codex-router-proxy/src/maintenance_actor.rs` declaration/source-guard path plus new private `maintenance_actor/maintenance_behavior_tests.rs`. Parent production lines1–539 and test-only `MaintenanceCompletion` lines69–87 remain; old inline tests lines544–1079 move as complete items; all12 test names/attributes/fixtures/helpers preserved. `server.rs` and proxy test callers retain parent completion type. No implementation applied.

Planning decision: this is the smallest coherent mechanics-only cut now that baseline package compilation is available. Stop conditions are public path/visibility changes, fixture/assertion behavior changes, MCP test-owner collision, or any required mechanism beyond module placement/source-guard relative path. Next execution step, only when proceeding, is red/green list and focused test around this map, then formatting/Clippy/package proof; final CI remains last.

Checkpoint commit `maintenance actor cut map` is staged but not committed: signed `git commit -m 'Map maintenance actor test extraction'` produced no output for ~15 seconds and was interrupted with exit130. No unsigned fallback or signing configuration change was attempted. Exact staged paths remain only the cut map and trace; current blocker is signing availability/Touch ID completion, not source planning.

## Maintenance actor cut applied and verified — 2026-10-04

Applied the exact map in `docs/wip/2026-10-04-maintenance-actor-cut-map.md`. Production parent prefix lines1–539 remained byte-identical; child `crates/codex-router-proxy/src/maintenance_actor/maintenance_behavior_tests.rs` now owns the 12 existing tests/helpers. Parent is543 lines; child532 formatted lines. Only source guard path changed to `include_str!("../maintenance_actor.rs")`; `MaintenanceCompletion` stayed parent-owned for server/proxy test callers. MCP test owner remains untouched and its fixes reservation is released.

Final focused validation: `cargo test -p codex-router-proxy maintenance_actor:: -- --list` exit0, all12 discovered; `cargo test -p codex-router-proxy maintenance_actor::` exit0,12 passed/0 failed/476 filtered/0.06s. `cargo fmt --all -- --check` exit0; `cargo check -p codex-router-proxy --locked` exit0/0.26s; `cargo clippy -p codex-router-proxy --all-targets --locked -- -D warnings` exit0/0.23s. Formatted body comparison exit0 proved moved child equals original inline test body except source-guard path. Size checker exit1 with27 remaining oversized files; maintenance actor removed from violations.

No CI/Cargo/SQLx/migration/public API/security/production edits. Whole refactor/final gate/review/PR remain incomplete. The current docs/cut map and source changes are staged with prior uncommitted planning docs; signed commit remains blocked by prior signer hang (no unsigned fallback).

Signed commit attempt for the applied maintenance cut produced no output for about six seconds and was interrupted with exit130. This is the second signing attempt to block/hang; no unsigned fallback, signing config change or trust bypass was used. The exact four-file cut/validation checkpoint remains staged but uncommitted.

## Blocker classification and A-class correction — 2026-10-04

Applied owner classification: **A** stale artifact claim, corrected now; **B** no unresolved architecture/design decision surfaced for the applied mechanics-only cut; **C** no security/auth decision involved; **D** deliberate hold remains on whole-goal canonical admission/final CI/remaining27 Rust files and on signing bypass.

A-class correction: `docs/wip/2026-10-04-maintenance-actor-cut-map.md` opening now distinguishes pre-cut1080 lines from applied parent543 lines, matching its applied-result section. No source behavior changed. Re-run document link/privacy/whitespace checks before any checkpoint attempt.

Signing remains a recoverable workflow blocker but cannot be bypassed: owner boundary forbids unsigned fallback and signing configuration changes. No new commit attempt in this correction.

## A-class signing recovery resolved

Per liaison correction, repo-local instructions permitted `git commit --no-gpg-sign` after two blocked signer attempts and no stricter repository prohibition exists. Committed the staged validated cut as `96f57779` using that scoped fallback; hooks remained enabled, no trust/config/auth change, push or merge. Working tree was clean immediately after commit. The prior signing blocker is resolved as an authorized workflow fallback; whole-goal hold and remaining27-file plan/CI/review work remain unchanged.

## Control connection cut applied and verified — 2026-10-04

Applied second bounded slice from `docs/wip/2026-10-04-control-connection-cut-map.md`: moved the three inline admission tests into private `crates/collaboration-service/src/control_connection/admission_error_tests.rs`; kept `wake_creation_crash_tests.rs` and production handler unchanged. Production prefix byte-identical; parent902 lines/child129 formatted lines.

Final validation: `cargo fmt --all -- --check` exit0; test list exit0/3 discovered; focused `cargo test -p collaboration-service control_connection::admission_error_tests::` exit0/3 passed/0 failed/214 filtered (integration binaries had no matching tests); `cargo check -p collaboration-service --locked` exit0/5.04s; package Clippy all-targets -D warnings exit0/11.62s. Size checker exit1 with26 remaining oversized files; control_connection removed.

No API/Control JSON/schema/dispatch/wake/SQLx/migration/CI/production changes. This slice is uncommitted; next signed local checkpoint may use authorized unsigned fallback only after blocked signer attempts. Whole refactor/canonical plan/review/PR remains incomplete.

## Supervisor test cut applied and verified — 2026-10-04

Owner coordination01a108ea supersedes the prior corroboration pause and directs same-Lead proactive continuation of the original pure decomposition scope. Applied next admitted mechanics-only cut: moved four typed-failure tests from `external_provider_supervisor/tests.rs` into private `failure_projection_tests.rs`; parent fixtures and provider supervisor production code remain unchanged. Exact map: `docs/wip/2026-10-04-supervisor-tests-cut-map.md`.

Proof: fmt exit0; four child tests discovered and passed (0 failed, 140 filtered; integration binaries had no matching filters); host package check exit0/22.21s; host all-target Clippy -D warnings exit0; source diff check exit0. Strict size inventory now has25 remaining oversized files. No API/provider semantics/fixture/CI/Cargo/SQLx/migration/auth/security/production changes.

This third cut is not yet committed; commit it as the next local checkpoint using the already authorized post-two-signer fallback. Whole canonical plan/final CI/review/PR still incomplete.

## Approval dispatch cut applied and tested — 2026-10-04

Owner completion criterion now targets a draft PR or genuine blocker; continued mechanics-only work is authorized by coordination01a108ea and the retained stop-review instruction. Applied next cut from `docs/wip/2026-10-04-approval-dispatch-cut-map.md`: moved three permission-outcome tests from `approval_dispatch_tests.rs` into private `permission_outcome_tests.rs`. Parent fixtures/scripts remain; no ACP/API/JSON/auth/CI changes.

Focused proof: three child tests passed/0 failed/141 filtered; integration binaries had no matching tests. Test discovery compile exit0. Formatting and package quality checks are next for this cut. Whole canonical plan, final CI, independent review and draft PR remain incomplete.

## Quota refresh helper cut applied and verified — 2026-10-04

Applied third continuing mechanics cut from `docs/wip/2026-10-04-quota-refresh-cut-map.md`: moved trailing quota-floor notification/generation/diagnostic/error helpers and freshness test into private `quota_refresh_helpers.rs`, with demonstrated `pub(super)` access only for parent-used type/functions. Parent is913 lines/child182 lines.

Validation: fmt exit0; exact freshness test with required `keychain-test-support` exit0/1 passed; `cargo check -p codex-router-cli --locked` exit0/16.04s; all-target Clippy with feature exit0/29.88s. Initial focused invocation without the required feature failed at the repository's existing compiled CLI acceptance guard; no source issue, corrected by established CI feature. Size checker exit1 with23 remaining oversized files; quota service removed.

No public API/quota persistence/error/concurrency/auth/CI changes. Slice is not yet checkpoint-committed; next action is exact preservation review, stage and local checkpoint. Whole PR/review/CI still incomplete.

## Quota HTTP Claude test cut applied and verified — 2026-10-04

Applied next test-only cut from `docs/wip/2026-10-04-quota-http-claude-cut-map.md`: moved the Claude recovery provider/double and idle-account test into private `quota_http_claude_tests.rs`; parent is991 lines/child260 lines.

Proof: compiled discovery corrected the module path; final focused command with required `keychain-test-support` passed2/0 failed/420 filtered. Package check exit0/1.70s, all-target Clippy with feature exit0/5.69s, fmt exit0. Size checker exit1 with22 remaining oversized files; quota_http_tests removed. No API/HTTP/schema/auth/SQLx/CI/production changes.

This slice remains uncommitted pending staging/checkpoint; whole PR/review/CI incomplete.

## Selection projection cut applied and verified — 2026-10-04

Applied next mechanics-only cut from `docs/wip/2026-10-04-selection-projection-cut-map.md`: moved the complete inline selector test module into private `selection_projection_tests.rs`; parent is896 lines/child611 lines. Added/fixed only the test-only child gate and duplicate-import cleanup.

Proof: fmt exit0; 9 selector tests passed/0 failed/155 filtered; state check exit0/3.95s; state all-target Clippy -D warnings exit0; size checker exit1 with21 remaining oversized files; selector projection removed from violations. No selector behavior/API/SQLx/migration/CI/production changes.

## Credential renewal outcome cut applied and verified — 2026-10-04

Applied next test-only cut from `docs/wip/2026-10-04-credential-renewal-cut-map.md`: moved four renewal/retry scenarios plus `RejectingRefreshClient` into private `renewal_retry_tests.rs`; parent is933 lines/child421 lines.

Proof: four renewal tests passed/0 failed/75 filtered/1.10s; auth check exit0/4.45s; auth all-target Clippy -D warnings exit0/4.90s; fmt exit0; source-prefix/test-name preservation passed. Size checker exit1 with20 remaining oversized files; auth outcome test removed. No credential schema/storage/auth/network/security/CI changes.

## Whole-work mechanics plan admitted — 2026-10-04

Owner delivery criterion authorizes this lane toward its own draft PR; stop-review continuation explicitly authorizes remaining pure mechanics. Created current mechanics-only plan (ignored working artifact) at `tmp/plan-workflows/2026-10-04-rust-file-decomposition.md`, planned-at HEAD `32b969b1`, terminal draft PR unmerged, one-pr topology. No product obligation/contract/state/failure/security/proof seam is invented; exact source maps and existing gates govern.

Current committed slices: maintenance actor `96f57779`; control connection `df21ac52`; supervisor failure projection `db75738a`; approval permission outcomes `01aa1bae`; quota helper `936772fc`; quota HTTP Claude `dbffedf1`; selector projection `bd3bd4d5`; credential renewal retry `32b969b1`; checker `958bf6c5`. Working tree clean before this trace update. Size checker reports20 remaining oversized files.

Required whole gates in plan: checker exit0, all-target workspace Clippy -D warnings with keychain feature, nextest workspace/quota harness, SQLx metadata, deny/audit, CLI builds/install, focused slice proofs, source/test inventory, independent review, then own draft PR CI/head verification. CI remains unedited; final integration last. No merge/release/production restart.

## Interaction broker test cut applied and verified — 2026-10-04

Applied the interaction broker split from `docs/wip/2026-10-04-interaction-broker-cut-map.md`: shared parent fixtures remain; decision/history tests moved to one private child and question lifecycle tests to another. Parent902 lines; children616/564. Existing turn-cancellation sibling retains `session`/`fixture_broker` visibility.

Proof: fmt exit0; collaboration-service lib tests exit0/27 passed/0 failed/190 filtered; package check exit0; Clippy all-targets -D warnings exit0/9.61s; size checker exit1 with19 remaining oversized files; parent broker test file removed. No interaction semantics/API/storage/CI/production changes.

## Thread participant integration cut applied and verified — 2026-10-04

Applied next test-only cut from `docs/wip/2026-10-04-thread-participants-cut-map.md`: split subscription/handoff tests and corruption/archived/unread tests into two private child modules while retaining the shared BoardStore fixture parent. Parent647 lines; children420/383.

Proof: fmt exit0; full thread_participants integration target exit0/13 passed/0 failed/0 filtered/0.76s; package check exit0/5.01s; package Clippy all-targets -D warnings exit0/8.81s. Size checker exit1 with18 remaining oversized files; thread_participants removed. No BoardStore API/SQLite/schema/participant semantics/CI/production changes.

## Permission entry path cut applied and verified — 2026-10-04

Applied next real integration-test cut from `docs/wip/2026-10-04-permission-entry-cut-map.md`: moved seven CLI/provider/Streamable HTTP permission scenarios into private `real_entry_tests.rs`; parent619 lines/child698 lines.

Proof: fmt exit0; collaboration-mcp lib focused tests exit0/7 passed/0 failed/82 filtered/0.42s; package check exit0/11.52s; all-target Clippy -D warnings exit0/26.94s. Size checker exit1 with17 remaining oversized files; permission_entry_path_tests removed. No MCP API/schema/broker/auth/storage/CI/production changes.

## ACP create-settings-order test cut applied and verified — 2026-10-04

Applied the bounded test-only split recorded in `docs/wip/2026-10-04-create-settings-order-cut-map.md`: moved the three trailing setting-outcome tests into private `settings_tail_tests.rs`; the parent retains shared ACP subprocess fixtures and all earlier ordering/gating tests. Parent is918 lines and child122 lines. Prefix comparison against the pre-cut parent is exact through old line916, and all three moved test names are preserved.

Proof: `cargo test -p acp-client-runtime --test create_settings_order` exit0 with12 passed/0 failed; `cargo fmt --all -- --check` exit0; package check `cargo check -p acp-client-runtime --locked` exit0; all-target Clippy `-D warnings` exit0. The first Clippy run caught and the bounded correction removed an orphaned doc comment; the rerun passed. Strict checker exits1 with16 remaining oversized files and no longer lists `create_settings_order.rs`.

This slice changes no ACP/API/setting semantics, subprocess fixtures, persistence, auth/security, CI, or production behavior. The exact five-path checkpoint (two Rust paths, cut map, trace) is staged next for the authorized `--no-gpg-sign` fallback after two prior signer hangs. No push, merge, release, production restart, or final CI gate yet.

Board discovery remains `no-home: no project for codex-router.rust-file-decomposition`; the only listed project/board is unrelated `shravan-claw`, so this local trace remains the shared record until a repository board association is supplied.

## Conversation fault identity test cut applied and verified — 2026-10-04

Applied the bounded integration-test split recorded in `docs/wip/2026-10-04-conversation-fault-identity-cut-map.md`: moved the two identity/`--from` conversation tests into private `identity_override_tests.rs`; the parent retains shared CLI result parsers, ACP/control fixtures, response-loss tests and resumed-prompt coverage. Parent is850 lines and child428 lines. Prefix comparison through old line848 and child-tail comparison through old line1274 are exact, with only the child `use super::*` import added.

Proof: `cargo test -p agent-collaboration --test conversation_fault_entry_paths` exit0 with9 passed/0 failed; package check exit0; all-target Clippy `-D warnings` exit0; fmt exit0. Strict checker exits1 with15 remaining oversized files and no longer lists `conversation_fault_entry_paths.rs`.

This slice changes no CLI/SessionRef/ACP/control semantics, persistence, auth/security, CI, or production behavior. It is the next local checkpoint; staging and commit follow after this trace/map write. No push, merge, release, production restart, or final CI gate yet.

## Provider conversation common-operation test cut applied and verified — 2026-10-04

Applied the bounded integration-test split recorded in `docs/wip/2026-10-04-provider-conversation-common-cut-map.md`: moved the first six provider common-operation tests into private `common_operation_tests.rs`; the parent retains shared constants/helpers and the remaining Codex/provider/unavailable/ignored-Cursor scenarios. Parent is857 lines and child465 lines. Source comparison against the pre-cut file is exact apart from module wiring and formatter removal of the old terminal blank line.

Proof: `cargo test -p agent-collaboration --test provider_conversation_cli` exit0 with11 passed/0 failed/1 ignored (the pre-existing authenticated Cursor test); package check exit0; all-target Clippy `-D warnings` exit0; fmt check exit0. Strict checker exits1 with14 remaining oversized files and no longer lists `provider_conversation_cli.rs`.

This slice changes no provider CLI/operation identity/fixture transport semantics, persistence, auth/security, CI, or production behavior. It is the next local checkpoint; staging and commit follow after this trace/map write. No push, merge, release, production restart, or final CI gate yet.

## Provider ACP failure test cut applied and verified — 2026-10-04

Applied the bounded integration-test split recorded in `docs/wip/2026-10-04-provider-acp-failure-cut-map.md`: moved the final provider-process transport and live-peer recheck tests into private `provider_failure_tests.rs`; the parent retains shared route fixtures and the first six delivery scenarios. Parent is967 lines and child158 lines. Prefix comparison through old line965 and child-tail comparison through old line1121 are exact, with only the child `use super::*` import added.

Proof: `cargo test -p codex-router-host --test provider_acp_delivery_route` exit0 with8 passed/0 failed; package check exit0; all-target Clippy `-D warnings` exit0; fmt exit0. Strict checker exits1 with13 remaining oversized files and no longer lists `provider_acp_delivery_route.rs`.

This slice changes no provider ACP route/delivery effect/operation-store semantics, subprocess fixtures, persistence, auth/security, CI, or production behavior. It is the next local checkpoint; staging and commit follow after this trace/map write. No push, merge, release, production restart, or final CI gate yet.

## Codex app-server generation test cut applied and verified — 2026-10-04

Applied the bounded integration-test split recorded in `docs/wip/2026-10-04-codex-app-server-generation-cut-map.md`: moved the first three generation/retirement scenarios into private `generation_tests.rs`; the parent retains held-empty-thread and scheduled-run scenarios plus shared fixtures. Parent is617 lines and child571 lines. Prefix/suffix comparison against the pre-cut source is exact; the child body preserves all moved tests with only formatter removal of the old terminal blank line.

Proof: `cargo test -p collaboration-service --test codex_app_server_delivery_route` exit0 with9 passed/0 failed; package check exit0; all-target Clippy `-D warnings` exit0; fmt check exit0. Strict checker exits1 with12 remaining oversized files and no longer lists `codex_app_server_delivery_route.rs`.

This slice changes no Codex app-server route/generation/evidence/reconciliation semantics, WebSocket fixtures, persistence, auth/security, CI, or production behavior. It is the next local checkpoint; staging and commit follow after this trace/map write. No push, merge, release, production restart, or final CI gate yet.

## External provider supervisor integration-test cut applied and verified — 2026-10-04

Applied the bounded split recorded in `docs/wip/2026-10-04-external-provider-supervisor-test-cut-map.md`: moved the settings projection test into `settings_projection_tests.rs` and the eleven lifecycle/operation tests into `lifecycle_tests.rs`; the parent retains all shared fixtures, macros and helpers. Parent is568 lines and children234/935 lines. Retained source and lifecycle child comparisons are exact; the settings child only drops the pre-cut terminal blank line under formatting.

Proof: `cargo test -p codex-router-host --test external_provider_supervisor` exit0 with12 passed/0 failed; package check exit0; all-target Clippy `-D warnings` exit0; fmt check exit0. Strict checker exits1 with11 remaining oversized files and no longer lists `external_provider_supervisor.rs`.

This slice changes no external-provider operation/effect/reconciliation semantics, subprocess fixtures, persistence, auth behavior, CI, or production behavior. It is the next local checkpoint; staging and commit follow after this trace/map write. No push, merge, release, production restart, or final CI gate yet.

## External provider runtime test cut applied and verified — 2026-10-04

Applied the bounded test-module split recorded in `docs/wip/2026-10-04-external-provider-runtime-test-cut-map.md`: moved admission/lifecycle tests into `admission_lifecycle_tests.rs` and MCP/shutdown/cancellation/output tests into `mcp_shutdown_tests.rs`; the parent retains shared fixtures and early classification/session tests. Parent is589 lines and children571/566 lines. Prefix and MCP child comparisons are exact; the admission child only drops the pre-cut terminal blank line under formatting.

Proof: `cargo test -p codex-router-host external_provider_runtime::tests:: --lib` exit0 with35 passed/0 failed/2 ignored; package check exit0; all-target Clippy `-D warnings` exit0; fmt check exit0. Strict checker exits1 with10 remaining oversized files and no longer lists `external_provider_runtime/tests.rs`.

This slice changes no external-provider runtime/API/ACP/subprocess semantics, persistence, auth behavior, CI, or production behavior. It is the next local checkpoint; staging and commit follow after this trace/map write. No push, merge, release, production restart, or final CI gate yet.

## MCP HTTP listener test cut applied and verified — 2026-10-04

Applied the bounded listener-test split recorded in `docs/wip/2026-10-04-mcp-http-listener-test-cut-map.md`: moved the real initialization scenario into `initialization_tests.rs` and eight response-loss/observation scenarios plus their local replay helper into `response_loss_tests.rs`; the parent retains bind/drop/cancellation, schema/origin/IPv6 tests, and shared protocol helpers. Parent is573 lines and children408/725 lines.

The first extraction attempt exposed that `protocol_response_json` and `initialize_mcp_session` were also used by parent and initialization tests. They were returned to the parent shared harness, preserving ownership and avoiding visibility widening. Final proof: listener subtree tests exit0 with17 passed/0 failed; package check exit0; all-target Clippy `-D warnings` exit0; fmt check exit0. Strict checker exits1 with9 remaining oversized files and no longer lists `mcp_http_listener_tests.rs`.

This slice changes no MCP HTTP/API/cancellation/session/response-loss semantics, persistence, auth behavior, CI, or production behavior. It is the next local checkpoint; staging and commit follow after this trace/map write. No push, merge, release, production restart, or final CI gate yet.
