# Rust file decomposition proposal

Whole-work planning result: **blocked**. Canonical whole-plan identity: **none**. The independent Python checker is implemented and verified. This proposal preserves candidate scope and proof obligations; it does not authorize held Rust implementation or CI integration.

[Current trail](work-trails/2026-10-03-rust-file-decomposition/main.md) records checkpoint scope, evidence, and remaining work. [Board citation map](2026-10-03-board-source-citation-map.md) preserves unchanged source anchors. Detailed local research and coordination records remain in ignored scratch.

## Goal and boundaries

Decompose every repository Rust source over 1000 physical lines into meaningful responsibilities within its existing crate, and add an error gate to the existing lint pipeline. Keep public API paths, behavior, stored formats, network/auth/security boundaries, concurrency, error/effect classification and explicit tracing targets unchanged. No generated/vendor exclusions, grandfather list or warning mode is admitted.

Inventory at source baseline `7a8cbd8943e6fb2dac23bcde7069203925003918`: 1640 tracked Rust files, 28 oversized, 11 crates. All oversized sources are LF-terminated. Whole-file and caller inspection remains required before each exact cut is admitted.

## Implemented independent checker

Files: [checker](../../scripts/tooling/check-rust-file-size.py) and [permanent tests](../../scripts/tests/test_rust_file_size.py). The checker counts LF bytes plus an unterminated final line; 1000 passes and 1001 fails. Blank/comment/string lines count. CRLF counts once per LF; Unicode separators do not replace physical LF.

The real Git inventory includes tracked and new unignored `.rs` paths, deduplicated and NUL-safe; tracked ignored sources remain included. Deterministic JSON-escaped diagnostics identify every oversized source. Exit 0 means compliant, 1 means violations, and 2 means incomplete enumeration/read. No Rust dependency or network access is required.

Permanent real-Git/CLI tests cover line boundaries, current worktree bytes, ignored artifacts versus tracked sources, missing tracked files, non-repository invocation, subdirectory invocation, literal unusual filenames and absence of generated/vendor/test/prototype exemptions. All 16 tests pass after relocation. The full repository checker currently exits 1 with the 28 baseline violations; this is expected enforcement evidence, not repository compliance.

## Candidate first Rust extraction

`crates/codex-router-proxy/src/maintenance_actor.rs` has 1080 lines: its real production contract/actor/coalescing/worker remains in the root, while the 12 existing scenario tests form a private companion `maintenance_actor/maintenance_behavior_tests.rs`. Both files are expected below 600 lines after formatting, measured during implementation.

Move complete items with attributes and literal bytes intact. Preserve all tests, assertions, counters/locks, repository doubles and cleanup. The source guard's moved `include_str!` path must read `../maintenance_actor.rs`, so it still examines the same production hint enum and repository-boundary marker. No source guard or predicate may be removed or weakened.

Candidate characterization commands, **not run**: `cargo test -p codex-router-proxy maintenance_actor:: -- --list` and `cargo test -p codex-router-proxy maintenance_actor::`. Verify all 12 cases in compiled pre/post discovery; record intentional module qualification changes. The module filter includes cleanup/compaction leaf names that omit maintenance. Real SQLite and actor lifecycle behavior must still pass. Rust source writes remain held pending authorized resume and actual compile-proof feasibility.

## Responsibility-based strategy

| Family | Candidate responsibilities | Required preservation |
| --- | --- | --- |
| Proxy server/HTTP | listener/runtime composition, transport contracts, authenticated preparation, Hyper body completion, protocol error rendering, scenario suites | public module paths, byte forwarding, retries, reservation lifetime, tracked tasks/shutdown, real TCP/SQLite proof |
| DB write actor | command/repository boundary, queue processing/health, affinity debounce scheduler, scenario suites | queue independence/capacities, bounded shutdown, clocks, degraded state |
| Host | supervisor admission/settlement, backend operations, publication/lifecycle implementation, capability/permission/cancellation tests | public reexports, effect/identity semantics, fixture paths/macros, exact ACP lifecycle |
| State/CLI/auth | projection adapter/facts/estimator, account parsing/login/import/mutations, quota floor notifications, pruning/recovery/renewal and HTTP tests | freshness/generation consistency, error copy, SQLx/encrypted store, existing runtimes |
| MCP/service/CLI/board | structured result conversion outside the existing tool-router block, catalog/schema/push suites, Control admission, broker history, CLI faults, participant/subscription/corruption suites | registry/golden schemas, sibling fixtures, compiled CLI, permissions, invalid stored-value rejection |
| Installed Codex harness | scenarios, isolated roots, state seeding, child lifecycle, transcript redaction, protocol fixtures, overlap/quota state machine, tests | shell filters, ignored real-path cases, parent-private dependencies, manifest paths, literal bytes and isolated real journeys |

These are candidate research boundaries, not approved exact destination maps. Keep current roots as real API/composition owners; use private children and existing public reexports where definitions move. Prefer descendant inherent implementations without wrapper paths. Widen private visibility only for demonstrated sibling consumers. No new runtime layers, behavior fixes, schemas or dependencies are admitted.

## Delivery and proof obligations

Proposed grouping: one PR containing checker, complete refactor and enabled CI error gate; unmerged terminal. Enable the checker and its tests in the existing CI lint job after all sources comply, preserving Clippy, SQLx and every other gate. A separate early enforcement PR would fail on the baseline; weakening or grandfathering the gate is not allowed. CI remains unchanged in this checkpoint.

| Obligation | Observation | Current state |
| --- | --- | --- |
| Strict physical-line limit | fixed 1000/1001 CLI cases and full repository scan | checker verified; repository remains oversized |
| Meaningful pure extraction | full-item/literal preservation, public path and owner/test/filter maps | inventory and candidate boundaries only |
| Existing behavior preserved | compiled scenario inventory, real focused component tests and workspace suites | Rust proof not established |
| Existing lint pipeline integration | real size check and unchanged Clippy/SQLx gates | not implemented |
| PR-ready-unmerged delivery | Lead assessment, independent review, checks/comments/threads/head/mergeability | not begun |

Keep every existing test and required gate. Repair only moved imports/paths/qualification maps; remove none and duplicate no fixtures. Preserve formatting, script/bootstrap/release automation tests, dependency policy/audit, workspace/all-target Clippy, SQLx metadata verification, CLI build variants/install and quota harness builds, workspace/keychain nextest and quota harness. Add existing isolated real-path proof where ordinary automated tests do not establish moved runnable behavior. No production process replacement.

Stop for changed ownership/public contract/storage/security semantics, new runtime mechanisms, missing proof, source drift, undiscovered tests, rewritten multiline literals, weakened guards, or refreshed snapshots hiding behavior changes. No stand-ins are selected.

## Current ownership, holds and continuation

Host and fixes tracks cleared the exact 28-file pure reorganization and checker surfaces; selector reported no relevant write reservation. Board docs-only non-overlap was reported through coordination. Preserve future overlapping feature/proof handoffs and actual old-to-new source/test/filter maps before other writers re-anchor. These clearances are not implementation acceptance.

The owner currently authorizes only local checkpoint commits of existing work. No push, merge, extra implementation, security changes, Rust/CI writes or dependency/network/toolchain attempts are authorized by that checkpoint instruction. Additional feature work remains on hold.

Historical dependency evidence: pinned Rust/Cargo versions and formatting passed; offline check failed before compilation because `chrono` was unavailable; online index waits were interrupted after slow-transfer warnings. Causes and current dependency availability are unverified; do not claim a firewall diagnosis.

Next narrow step when the owner lifts the relevant hold: verify one bounded pinned-toolchain baseline check, then settle exact cut maps and admitted mechanics before creating the sole executable whole-plan record. Do not infer a ready plan from this checkpoint or the passing checker tests.

## Environment report at checkpoint

Coordination reports that an updated sandbox policy cleared dependency access elsewhere. Current worktree compile/proof was not re-run, and the feature-plan hold remains. Treat the historical missing-cache/index-stall observations as historical, not a confirmed current infrastructure failure or firewall diagnosis. The existing checker is checkpointed in `958bf6c5`; source extraction and final CI integration are not implemented.

## Updated overlapping owner reservation

The fixes track has resumed its separately authorized native-interrupt work and temporarily reserves `crates/collaboration-mcp/src/mcp_server/tests.rs` for real propagation tests. Defer decomposition of that exact test owner until its per-owner handoff supplies current source/test/filter evidence. Other previously cleared oversized-file surfaces remain cleared; this reservation and the decomposition feature-plan hold are distinct. No source edit or build follows this notification.
