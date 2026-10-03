# Split oversized files — Phase 1 module map

Status: Phase 2 GO; #115/main fast-forwarded to `609a0b02780d8cff8aa6f0be367d37370ef9aa92`. Lead approved the credit-owner delta before source moves.

Baseline: `dea647b545f4ed5c9cb22565856c196a3c36ac98`, branch `refactor/split-oversized-files`, worktree `/Users/shravansunder/Documents/dev/project-dev/codex-router.refactor-split-oversized-files`.

Tracker: Linear LUNA-407. Commissioner: Claude session `8d47f947-6858-41d1-a720-817d49e2b549`. Implementing Sidekick: Codex session `01a0fe50-6993-76b1-8f30-675ba8a4e612`.

## Trace

- 2026-10-02: verified clean worktree, branch and exact baseline; read repository AGENTS.md. Source remains read-only during Phase 1.
- 2026-10-02: commissioned six independent native Workers for complete item/range/fixture inventories, one per file. Classification: Complete direction, Local span, Task horizon; bounded source-inventory cut; Workhorse Luna xhigh. The Sidekick decides proposed module boundaries and verifies their decisive evidence.
- 2026-10-02: Lead explicitly instructed no board root or further board discovery. Direct messages only; this proposal and later commits are the authorized trace. Discovery stopped; no board was created or posted to.
- 2026-10-02: measured six files at 43,799 lines. Both crate roots are predominantly tests. No `target/debug` exists in this worktree, so there are no local debug artifacts to clean during analysis.
- 2026-10-02: Lead approved source-input adaptations for both source-scanning guards, retaining original checks and adding nonempty/root-presence checks. Approval applies to future Phase 2 implementation, not source moves now.
- 2026-10-02: verified six Worker inventories against decisive sources and reconciled all 391 direct test destinations. A range-coverage script checked each existing marked test against one target suite; exit 0, no missing or duplicate destinations. Adjusted source-range starts to include test attributes. Kept three mutually calling audit scenarios together; kept rollup interval queries with rollup computation. Source/config diff against baseline remains empty.
- 2026-10-02 stop-review checkpoint: checked PR #115 directly through GitHub CLI (exit 0); state OPEN, mergedAt null, mergeCommit null. The merge prerequisite is still unmet, independently of the pending Lead go-ahead. The proposal has been delivered through Router; supported incoming direct replies arrive as session messages. No board listener was created, per the Lead's explicit no-board instruction. Waiting on these explicit commission gates, not an optional unanswered question.
- 2026-10-02 20:59 UTC: saved one active, one-shot follow-up wake for this exact Sidekick session through the collaboration waiting path. Wake `01a0fe6a-7f24-7321-8bdb-24ed533d1a88`, due `2026-10-02T21:09:47.364Z`, expires `2026-10-02T21:29:47.364Z`. Scheduling is confirmed; firing/native acceptance is not yet observed. On delivery, process Lead review first; implementation still needs explicit go plus #115 merged. Cancel the obsolete wake if those dependencies arrive before it fires. No recurring schedule or board listener created.
- 2026-10-02: Lead reviewed and approved the map as the Phase 2 basis, requesting descriptive version-range filenames. Renamed the proposed files to `legacy_schema_v1to5.rs` and `legacy_schema_v6to10.rs`, preserving their jobs/ranges/budgets. Lead will DM the #115 merge commit; execution waits for that merged prerequisite. Cancelled wake `01a0fe6a-7f24-7321-8bdb-24ed533d1a88`; Router returned state cancelled, no pending delivery, no first firing and no dispatched deliveries. No replacement wake scheduled; incoming Lead DM is the continuation path.

## Scope and prerequisites

Structure only: preserve public paths, behavior, SQL, assertions, test attributes, fixture lifetime and synchronization. Each resulting source/test file must fit approximately 900 lines, preferably below 600 where responsibility permits. No new domain abstraction or compatibility implementation.

The Lead approved this module map as the Phase 2 basis. Phase 2 starts only after PR #115 merges to main; the Lead will send the merge commit by DM. Then merge main into this branch, refresh the map/inventories against that head, and make mechanical moves in coherent commits. Implementation and runtime proof have not started.

## Phase 2 baseline refresh — approved credit-owner delta

The execution baseline is `609a0b02780d8cff8aa6f0be367d37370ef9aa92` (PR #115, v0.1.62). Merge was a clean fast-forward with no conflicts. Historical ranges in the six sections below locate item identities at `dea647b5`; refreshed exact ranges/test inventories live in `tmp/module-maps/*-refresh.md` and the named baseline manifest under `tmp/refactor-proof/`. Preserve current `609a0b02` bodies, including every #115 change; do not restore pre-credit bodies.

Lead approved this delta by DM before moves:

- Retain `account_selection/account_admission.rs` (current live source/peer account admission owner) in place of the old `floor_switch_peer.rs`. Keep its current types, field names and private runtime couplings.
- Retain `websocket/account_turn_admission.rs` in place of `floor_switch_admission.rs`. Pump/tunnel fields and test fixtures retain current `AccountTurnAdmission` and `LiveAccountAdmissionAssessor` names and credit provenance; no old path/shim is recreated.
- Retain `burn_down/credit_assessment.rs`. Credit input/result/reason fields travel with the mapped input/result/reason homes; the current credit-assessment child keeps its job.
- Keep new proxy `src/tests/` credit transport/compact/affinity/credential-generation suites and WebSocket credit-turn suites in their existing logical trees; adapt only module-relative paths and fixture visibility required by moves. Existing credit test support remains one copy.
- Add `proxy_tests/credit_seed_fixtures.rs` for #115's existing credit/account/token seed functions; this keeps the quota seed fixture within budget and introduces no runtime owner.
- State selector reads now carry #115's one transaction through window/status/credit/maintenance observations. Preserve that transaction and exact SQL/bindings; selector-window owner remains unchanged. Credit-store production modules stay outside this refactor.

Lead explicitly reserved version/release authority: no version bump, tag or release in this refactor. Gate sequence remains format, workspace clippy, SQLx check, full CI nextest, equal before/after named-test inventories, then one PR. Host permission grant is needed for shared linked-worktree Git metadata and test sockets/process proof; those grants do not authorize production or user-state interaction.

The six refreshed source files total 45,012 lines (proxy/lib 13,511; websocket 7,294; account_selection 5,735; selection/burn_down 6,258; state/lib 6,150; sqlite 6,064). Existing separately extracted credit suites remain additional preserved tests, not folded into these line totals. The baseline source named-test manifest records 3,351 explicitly attributed cases across tracked workspace Rust files; the compiled nextest manifest also captures generated/parameterized cases and cfg selection.

Implementation admission: the explicit Phase 2 commission and reviewed map govern this mechanical refactor; requested terminal is one PR, unmerged. The commissioning Lead owns any structural delta and has approved the refresh above. This user-directed artifact home/delivery basis takes precedence over generic canonical-plan skill metadata requirements; no new planning authority, public contract or workflow cycle is inferred.

## Reading the map

All ranges below refer to the baseline above, not the eventual merged head. Estimated target sizes include imports and module declarations; they are budgets, not measured results. A row with several ranges collects existing items, not fragments of function bodies. Attributes, documentation, SQL literals and complete items travel together. Exact item inventories are retained under `tmp/module-maps/` for verification.

Use the existing public module as its declaration/API owner. A child can contain additional inherent `impl` blocks for a parent-owned type without wrappers or delegation methods. Keep fields private in the parent where that lets descendants access them; for leaf-owned records, widen only fields/functions actually consumed by siblings to `pub(super)`. Use `pub(crate)` only for existing consumers outside that subtree. When definitions must move, explicitly re-export them from their established public module; these exports constitute the module tree, not a second compatibility implementation. New children stay private.

Tests remain unit/integration scenarios in the existing crate binaries. Test files end in `_tests.rs`; fixture files have a named responsibility. A small test coordinator imports shared fixtures for its child suites. Preserve the existing shared counters, locks, channels, drop cleanup, Tokio attributes, panic allowances, and secret canaries. No duplicated fixture implementation or additional global state.

## 1. Proxy crate root — `crates/codex-router-proxy/src/lib.rs`

Current responsibilities: production declarations/package name at 1–28; test imports at 30–206; affinity recorders at 208–343; protocol, HTTP, auth and credential tests at 344–1325; selector integration tests at 1327–4594; listener/HTTP/runtime/WebSocket integration tests at 4595–9186; transport and state-seeding fixtures at 9188–10932; WebSocket routing/tunnel tests at 10933–13089. This is a test-ownership split; existing production modules remain their current owners.

Proposed tree: retain `src/lib.rs` (~35 lines), add `src/proxy_tests.rs` (~100, test coordinator), and these files beneath `src/proxy_tests/`. Ranges denote all complete tests/items in the stated span.

| Target file | Job and old locations | Approx. lines |
| --- | --- | ---: |
| `request_contract_tests.rs` | Package/auth/route/header/endpoint contracts, 344–495 | 190 |
| `http_forwarding_tests.rs` | Transport, byte/query preservation, supported routes, 496–740 | 290 |
| `http_auth_tests.rs` | Auth carriers, affinity secret and owner recording, 742–1104 | 420 |
| `credential_retry_tests.rs` | HTTP credential refresh/fallback/rejection and audit, 1105–1325 | 280 |
| `selector_projection_tests.rs` | Snapshot hydration, freshness and burn history, 1327–1706 + 1822–1904 | 530 |
| `quota_observation_tests.rs` | Runtime quota quarantine vs non-quota errors, 1707–1821 | 150 |
| `reservation_projection_tests.rs` | Release, cross-transport counts, stale reservations, 1905–2237 | 390 |
| `selector_affinity_tests.rs` | Sync/async durable affinity and normal fallback, 2238–2444 + 3102–3474 | 650 |
| `session_affinity_tests.rs` | Two-hour session reuse/expiry/persistence/precedence, 2445–2788 | 390 |
| `floor_switch_tests.rs` | Hard affinity and read-only peer switch assessment, 2789–3101 | 370 |
| `selector_metadata_tests.rs` | Bounded affinity parsing and per-window eligibility, 3475–3672 | 245 |
| `selector_hold_tests.rs` | Cooldown, preferred account and route-band fairness, 3673–3809 + 4263–4594 | 535 |
| `concurrent_selection_tests.rs` | Atomic starts, six-client pressure and near-reset drainage, 3810–4262 | 510 |
| `listener_adapter_tests.rs` | Bind/upgrade/HTTP parsing and bounded accept, 4595–5078 | 545 |
| `http_runtime_tests.rs` | Assembled state/secrets and served floor routing, 5079–5485 | 470 |
| `quota_replay_tests.rs` | HTTP fallback replay and unreplayable errors, 5486–5976 | 550 |
| `quota_exhaustion_tests.rs` | Multi-account exhaustion and active cooldown, 5977–6445 | 530 |
| `session_runtime_tests.rs` | Live affinity timestamp persistence, 6446–6660 | 255 |
| `audit_runtime_tests.rs` | Private audit and HTTP/WebSocket redaction scenarios that call each other, 6661–6799 + 7122–7368 | 435 |
| `audit_failure_tests.rs` | Scrubbed audit append failure, 6801–6833 | 65 |
| `sse_runtime_tests.rs` | Streaming and split-frame previous-response affinity, 6845–7120 | 330 |
| `websocket_dispatch_tests.rs` | Malformed frame pass-through and auth reload, 7370–7666 | 350 |
| `websocket_upgrade_tests.rs` | Fragmented upgrade, served floor and HTTP concurrency, 7668–8126 | 520 |
| `websocket_concurrency_tests.rs` | Legacy starvation reproducer and second client admission, 8128–8410 | 340 |
| `websocket_rejection_tests.rs` | Auth/path rejection, preconnect and affinity-task drain, 8411–8826 | 475 |
| `runtime_shutdown_tests.rs` | First-frame cancellation, active drain and continued acceptance, 8827–9186 | 420 |
| `websocket_routing_tests.rs` | Protocol and authenticated first-frame selection, 10933–11348 | 475 |
| `websocket_affinity_tests.rs` | Carrier rejection, durable continuation and refresh, 11349–11877 | 600 |
| `websocket_payload_tests.rs` | Large/future/nested payload pass-through, 11878–11951 | 110 |
| `tunnel_forwarding_tests.rs` | First/subsequent/control frames, close and handshake sanitization, 11953–12549 | 655 |
| `tunnel_affinity_tests.rs` | Owner recording, slow recorder, pinned multi-turn socket, 12550–13089 | 600 |
| `affinity_record_fixtures.rs` | Original secret providers and recording/blocking recorders, 209–342 | 165 |
| `loopback_request_fixtures.rs` | Request/response reads and WebSocket connections, transport-only items within 9188–9602 + 10407–10460 | 440 |
| `quota_seed_fixtures.rs` | Account/token/window/floor/history seeders, 9696–10194 | 540 |
| `affinity_seed_fixtures.rs` | Status/affinity waits and durable seeders, state-only items within 9188–9602 + 9623–9695 + 10195–10285 + 10347–10406 | 400 |
| `test_identity_fixtures.rs` | Counter, local auth, account IDs, clock, temp directory and must_ok, 208 + 9603–9622 + 10286–10346 | 140 |
| `selection_record_fixtures.rs` | Recording/rejecting/exclusion-aware selectors, 10498–10671 | 205 |
| `credential_record_fixtures.rs` | Sync/async resolver variants and refresh client, 10672–10903 | 265 |
| `upstream_record_fixtures.rs` | Recording/streaming/channel upstream and audit reporter, 6834–6844 + 10461–10497 + 10904–10931 | 110 |

Dependency direction: test coordinator → suites → named fixture modules → existing production APIs. Fixtures depend on other fixtures only for actual seed/transport work; they must not depend on test cases. Use explicit fixture imports when a broad parent import would create a cycle. Keep one `TEMP_COUNTER`, the exact `ProxyTestTempDir` cleanup and synchronization code. Fixture source ranges are deliberately noncontiguous because transport waits and stored-state waits are interleaved in the original file; cut at named function boundaries, not line-count boundaries.

Risky seams: `RecordingAffinityOwnerRecorder` and `BlockingAffinityOwnerRecorder` serve HTTP/SSE and tunnel scenarios; token/secret canaries and `ChannelUpstream` span suites; normal selector fixtures use cfg(test) sync repositories while concurrent scenarios use async stores. The original `clippy::panic_in_result_fn` crate test allowance remains at lib.rs. Runtime listener tests retain their existing real loopback/actor interactions and budgets.

There are 152 direct tests in this root. The audit-redaction test calls two other existing tests; all three move together into `audit_runtime_tests.rs`, preserving those calls without a test-to-test dependency across files. The largest individual test is the 247-line tunnel-dispatch scenario, so no test body needs to be divided.

## 2. WebSocket transport — `crates/codex-router-proxy/src/websocket.rs`

Current responsibilities: imports/limits/signals at 1–154; protocol, routing outcomes and affinity context at 155–330; authenticated sync/async routers and audit classification at 331–833; cancellation/session registry at 834–1294 and 1342–1359; transport close primitives at 1295–1341; registry tests at 1360–1403; forwarding tests/fixtures at 1405–5477; test-only blocking tunnel and production async tunnel at 5479–6031; duplex pumps/supervisor at 6032–6450; provider-error replacement at 6451–6623; completion/affinity/reservation state at 6624–6705; test-only blocking forwarding at 6706–6758; bounded metadata at 6759–7058; clocks at 7059–7071; blocking handshake at 7072–7118; frame/handshake conversion and tunnel errors at 7119–7188.

Retain `src/websocket.rs` as the public contract owner; children live in `src/websocket/`.

| Target file | One job and moves | Approx. lines |
| --- | --- | ---: |
| `../websocket.rs` | Public frame/handshake/decision/context/router/tunnel declarations and errors; protocol router impl, 155–359 declarations/impls, 5480–5508 declarations, 7159–7188; existing shared UpstreamToLocalPumpContext and ActiveTurnReservationState declarations | 385 |
| `authenticated_routing.rs` | Existing first-frame auth/select/credential loops plus rejection audit builders, router impls 363–777 + 787–833 + 136–154 | 510 |
| `session_registry.rs` | Registry/notifier records, registration, revocation, statistics and RAII cleanup, 834–1294 + 1342–1359 | 535 |
| `transport_cleanup.rs` | WebSocket configuration and close/reset semantics, 130–135 + 1295–1341 | 90 |
| `async_tunnel.rs` | Async builders, upgraded handshake, first-data wait and upstream headers, 5686–6031 + 7127–7147 | 410 |
| `duplex_forwarding.rs` | Both pumps, supervisor and active-turn reservation state, 6032–6450 + 6645–6705 | 550 |
| `provider_signals.rs` | Quota/capacity signal construction, HTTP selection-close mapping and error replacement, 108–129 + 778–786 + 6451–6623 | 245 |
| `response_metadata.rs` | Bounded auth/response metadata and forwarded owner recording, 6624–6644 + 6759–7058; relevant scan constants | 385 |
| `blocking_tunnel.rs` | Existing test-only blocking tunnel impl, forwarding, local handshake, frame conversion, 5510–5684 + 6706–6758 + 7071–7126 + 7148–7156 | 340 |
| `account_turn_admission.rs` | Existing serialized floor-switch owner, retained verbatim | 154 |
| `session_registry_tests.rs` | Existing two registry lifecycle tests, 1360–1403 | 55 |
| `forwarding_tests.rs` | Coordinator for 1405–5477 and existing external child tests; shared imports only | 150 |

Clock functions remain private in `websocket.rs` (~15 lines) for descendants; no shared clock utility is introduced. `router_websocket_config`, cleanup classification and existing crate-visible signal names remain accessible from their old module path through the necessary explicit tree exports. `blocking_tunnel.rs` is declared only under `cfg(test)`; do not promote synchronous IO into production.

Split the forwarding suite beneath `src/websocket/forwarding_tests/`:

| Target file | Old test/fixture batch | Approx. lines |
| --- | --- | ---: |
| `affinity_activity_tests.rs` | Recognized frame renewal and failed-forward preservation, 1496–1831 | 375 |
| `metadata_boundary_tests.rs` | Classifier, bounded metadata and completion/recorder ordering, 1838–1865 + 2181–2284 + 2692–2721 | 220 |
| `handshake_outcome_tests.rs` | Credential profile, shutdown before selection/connect and scrubbed preconnect errors, 2418–2691 + 2722–2797 | 400 |
| `idle_close_tests.rs` | Reset/close after completion and idle control frames, 2798–3048 | 300 |
| `turn_reservation_tests.rs` | Early release and same-socket fresh reservations/load, 3049–3441 | 440 |
| `quota_frame_tests.rs` | Quota signal vs oversized/non-error frames, 3442–3903 | 505 |
| `quota_persistence_tests.rs` | Immediate reconnect, queue latency and close-before-work, 3904–4381 | 525 |
| `alternative_selection_tests.rs` | Stalled/missing/short-exhausted alternative authority, 4382–4788 | 450 |
| `capacity_retry_tests.rs` | Capacity backoff/exhaustion and bounded thread ID, 4789–4960 | 210 |
| `unavailable_authority_tests.rs` | Queue and projection failures plus unchanged connection-limit observation, 4961–5331 | 415 |
| `shutdown_floor_tests.rs` | Active-pump shutdown and weekly floor notification, 5332–5476 | 185 |
| `provider_observer_fixtures.rs` | Existing observer variants/record state, 1832–1837 + 1866–2155 | 335 |
| `handshake_record_fixtures.rs` | Affinity recorder, selector/resolver/secret fixtures, 2156–2180 + 2285–2417 | 195 |
| `floor_switch_tests.rs` | Move existing external floor-switch tests, unchanged | 281 |
| `switch_terminal_tests.rs` | Move existing external terminal tests, unchanged | 342 |
| `switch_supervisor_tests.rs` | Move existing external supervisor tests, unchanged | 145 |
| `handshake_cancellation_tests.rs` | Move existing external handshake cancellation tests, unchanged | 122 |

Dependency direction: authenticated routing → protocol contracts, selection/credentials and metadata; async tunnel → routing + registry + duplex forwarding; duplex forwarding → registry, floor-switch admission, cleanup, metadata and provider signals. Registry owns cancellation-entry/statistics internals and its registration `Drop`; pumps borrow/clone the same registration and tokens as before. Existing floor-switch child tests continue to share `ImmediateSelectableFloorPeer` and `HeldSelectableFloorPeer` through their test coordinator. Move the existing UpstreamToLocalPumpContext declaration (6281–6297) and ActiveTurnReservationState declaration (6644–6649) into the parent; their impls remain with forwarding. Signals read the parent-owned context and never call pumps, avoiding a reverse dependency on the forwarding implementation. This moves existing records only, with no new signal props abstraction.

Risky seams: retain every biased `tokio::select!` arm and sequence; deliver terminal frame before releasing turn; release reservation before async owner recording; do not move registration drop ahead of joined pumps. FloorSwitchIntent epoch/watch/cancellation tokens remain the existing protocol. Parent-owned public structs keep private fields; registry registration fields used by the forwarding sibling need only `pub(super)`. External tests' relative `super::*` imports and `#[path]` attributes require rebasing. Existing audit fields and event messages remain identical; compiler-generated source locations naturally move.

Baseline inventory: 44 direct tests (two registry + 42 forwarding) and ten in the four external children. The latter share the existing fixed selector/resolver/secret fixtures; preserve that visibility through the coordinator. No individual production function is above 900 lines.

## 3. Burn-down policy — `crates/codex-router-selection/src/burn_down.rs`

Current responsibilities: constants/switch policy at 1–84; assessment inputs/window facts at 86–502; route policy at 503–532; assessment results/status/reasons at 533–1035; internal assessment records at 1036–1083; route composition at 1085–1257; account eligibility/floor/display metrics at 1258–1677; initial/far-idle admission at 1678–1801; window assessment/guard/weight policy at 1802–2104; routing explanation at 2105–2250; projection and ordering at 2251–2553; survival/math/default policy at 2554–2662; tests/fixtures at 2663–6086.

Retain `src/burn_down.rs` as the public module, with private children in `src/burn_down/`. Leaf public definitions are explicitly exported at the old `burn_down::*` path.

| Target file | One job and moves | Approx. lines |
| --- | --- | ---: |
| `../burn_down.rs` | Public constant names, module tree and existing API exports | 110 |
| `assessment_input.rs` | Route/account input and rejection facts, 86–296 | 240 |
| `window_facts.rs` | QuotaWindowFact constructors/accessors and status, 297–502 | 235 |
| `routing_policy.rs` | Existing configurable/default policy, floor-switch threshold and numeric policy arithmetic, 46–84 + 503–532 + 2604–2653 | 170 |
| `assessment_result.rs` | Result/account output, availability/pool/freshness/exclusion and limiting window, 533–862 + 1000–1042 | 420 |
| `routing_reasons.rs` | Existing reason enums, strings, context and assignment, 863–999 + 2105–2250 | 320 |
| `route_assessment.rs` | assess_route_band composition, initial/far-idle admission, pool matching/weights and profile check, 1085–1257 + 1678–1801 + 2064–2104 + 2654–2662 | 390 |
| `account_assessment.rs` | Existing assess_account, hard floor and display metrics, 1063–1083 + 1258–1677 | 490 |
| `window_assessment.rs` | Existing WindowAssessment, guards, pressure/survival/drain/salvage evidence, 1043–1062 + 1802–2063 + 2251–2269 + 2554–2603 | 395 |
| `candidate_priority.rs` | Existing comparator chain and same-pool equality, 2270–2553 | 330 |
| `burn_down_tests.rs` | Coordinator imports/constants for the existing suite | 60 |

Dependency direction: route assessment → account assessment, candidate priority and routing reasons; account assessment → window assessment and routing policy; all algorithms → input/fact/result/reason records. Record homes do not call orchestration. Preserve the full comparator chain, including tolerance and identity tie-breaks. Where record constructors invoke current small numeric functions, routing policy is the shared leaf; this moves those functions, not their formulas. No new public accessors are added to enable moves: exact internal field reads use `pub(super)` within `burn_down`.

Risky seams: algorithm constructs result records through struct literals and mutates their fields; field visibility must be narrowed to the burn_down subtree. Keep `SalvageSortKey` with the output record that stores it and allow only the window/comparator readers needed. Scenario harnesses mutate active-session counts and are shared by many tests; retain one copy. The preferred-next source guard requires the approved source-input adaptation described below.

The burn-down test coordinator loads these siblings beneath `src/burn_down/burn_down_tests/`:

| Target file | Existing batch and job | Approx. lines |
| --- | --- | ---: |
| `scenario_selection_fixtures.rs` | Original scenario structures and mutating harness, 2668–2838 | 205 |
| `quota_account_fixtures.rs` | Original account/window/assertion builders, 5931–6085 | 185 |
| `weekly_survival_tests.rs` | Constants, basic weighting, survival and early admission, 2840–3167 | 370 |
| `idle_admission_tests.rs` | Zero-weight admission and far-idle priority, 3168–3348 + 3528–3634 | 335 |
| `floor_switch_tests.rs` | Switch-band preference and hard stop, 3349–3527 | 215 |
| `window_guard_tests.rs` | Active confidence and short-window guard, 3635–3894 | 300 |
| `drain_selection_tests.rs` | Known/unknown weekly drainage and fallback, 3895–4264 | 410 |
| `runway_priority_tests.rs` | Reset-segment and forecast selection plus strict-order source guard, 4265–4531 | 320 |
| `quota_evidence_tests.rs` | Basic quota/status/freshness/eligibility cases, 4532–4992 | 500 |
| `minimum_runway_tests.rs` | Unknown-margin/near-zero survival behavior, 4993–5186 | 235 |
| `session_balance_tests.rs` | Six-session pool and S4/S5/S3n scenarios, 5187–5549 | 405 |
| `weekly_floor_tests.rs` | Safe label and configured-floor fail-closed behavior, 5550–5929 | 425 |

## 4. State crate root — `crates/codex-router-state/src/lib.rs`

Current responsibilities: production declarations/package name at 1–22; test imports/coordinator at 24–83; generation-maintenance scenarios at 85–646; migration fixture manipulation at 648–776; package/provider/session-affinity and floor/migration cases at 777–2168; source guard at 2170–2202; store/snapshot/history/session/projection/quota tests at 2203–5082; corruption/version/API/affinity/order cases at 5083–5462; temporary directories, account/observation/hash and legacy database builders at 5464–5988. Existing child `tests/credential_maintenance_store_tests.rs` already holds a separate 253-line suite and consumes parent fixtures.

Retain `src/lib.rs` (~30), including its existing independent `future_send_contract` test-module declaration at 5988–5989. Add `src/state_tests.rs` (~90 coordinator), and put the following beneath `src/state_tests/`:

| Target file | Job and old ranges | Approx. lines |
| --- | --- | ---: |
| `credential_claim_tests.rs` | Claims, reopen, purpose/provider validation and maintenance clocks, 92–646 | 595 |
| `session_affinity_tests.rs` | Provider-scoped migration/upsert/CAS/expiry/retention, 782–1188 | 450 |
| `provider_schema_tests.rs` | Immutable provider and missing index rejection, 1189–1272 | 120 |
| `floor_migration_tests.rs` | Floor validation and v10–v12 migration/rollback, 1273–1534 | 300 |
| `floor_schema_tests.rs` | Mutation-only/read-only schema gate, malformed columns and contention, 1535–1872 | 380 |
| `floor_mutation_tests.rs` | Bulk read, labels, bounded busy retry and commit-after-release, 1873–2168 | 335 |
| `storage_boundary_tests.rs` | Original source guard + package-name test, 2170–2202 + 777–781 | 85 |
| `store_projection_tests.rs` | Store/snapshot partition and selector parity, 2203–2426 | 265 |
| `read_only_tests.rs` | Read-only/schema/open/WAL behavior, 2427–2610 | 225 |
| `quota_history_tests.rs` | Durable history append/range/purge, 2611–2731 | 160 |
| `active_lease_tests.rs` | Acquire/release/prune/process-run identity, 2732–2906 | 215 |
| `session_history_tests.rs` | Retained completed events, compaction and clipped overlap, 2907–3197 | 335 |
| `session_terminal_tests.rs` | Schema counters, stale terminalization, migration/repair/retention, 3198–3585 | 430 |
| `burn_projection_tests.rs` | Rollup and per-connection burn normalization, 3586–3965 | 425 |
| `quota_exhaustion_tests.rs` | Partial evidence and runtime durable exhaustion, 3966–4273 | 350 |
| `quota_refresh_tests.rs` | Refresh/status staleness and legacy selector migrations, 4274–4696 | 465 |
| `credential_mutation_tests.rs` | Atomic alias-family invalidation, status/generation rejection, 4697–5032 | 380 |
| `repository_contract_tests.rs` | Code-review exclusion, corruption, unsupported versions and API usability, 5033–5205 | 215 |
| `response_affinity_tests.rs` | Hash-only route-scoped ownership, sync/async parity and purge, 5206–5415 | 250 |
| `account_order_tests.rs` | Stable account-list ordering, 5416–5462 | 80 |
| `test_identity_fixtures.rs` | Single TEMP_COUNTER, temp dir, account/observation/hash/error helpers, 85–90 + 648 + 5464–5564 | 160 |
| `policy_schema_fixtures.rs` | v10/v11/v12 manipulation and intact-policy check, 650–776 | 160 |
| `quota_schema_fixtures.rs` | v2/v3/v6 legacy database builders, 5565–5779 | 250 |
| `session_schema_fixtures.rs` | v8/v10/partial-v10 builders, 5780–5988 | 245 |
| `maintenance_store_tests.rs` | Existing external suite `tests/credential_maintenance_store_tests.rs`, unchanged | 253 |

Dependency direction: suites → named test fixtures → real async SQLx or cfg-gated sync fixture stores. Existing maintenance child moves under the coordinator without changing its cases. Fixture names and field access may require `pub(super)` only in the test subtree; the state crate API remains unchanged. Migration fixture functions continue to modify only test databases. Preserve the one counter and temp-directory cleanup; moving tests must not create collisions or change the account label/account ID scenarios.

Risky seams: multi-statement migration fixtures, manual writer contention and WAL/read-only assertions must remain exact. The source guard's input is approved to grow, not shrink. It currently truncates before the first `#[cfg(test)]` at sqlite.rs:4012, so scanning new files exposes previously later fixture references; preserve per-file truncation and the exact filter. Keep cfg-fenced imports on sync-only references after relocation; do not broaden production feature availability.

Baseline inventory: 79 direct tests (56 Tokio + 23 plain), four tests in the existing maintenance-store child and one in the unchanged `future_send_contract` child. These counts distinguish moved cases from pre-existing children.

## 5. SQLite store — `crates/codex-router-state/src/sqlite.rs`

Current responsibilities: imports/policy/schema literals and rebuild at 1–245; active-client/session records at 246–500; runtime-exhaustion row and store errors at 502–630; store/mutation declarations at 631–690; async lifecycle at 691–805; selector/exhaustion at 806–960; accounts/policy/snapshots/generation at 961–1339; affinity at 1340–1530 and 1849–1887; selector windows/status at 1531–1848; quota history at 1888–1989; active leases/session events/rollups at 1990–2571; narrow floor/status mutation store at 2572–2890; async repository traits/adapters at 2892–3094; sync fixture accounts/snapshots/selectors at 3095–4028; sync migrations at 4029–4723; sync repository adapters/affinity at 4724–4954; errors/transaction writes/row validation/staleness/math at 4955–5906, including two embedded tests.

Retain `src/sqlite.rs` as the public owner of stores/errors and lifecycle; private children live in `src/sqlite/`. Existing `account_migrations`, `account_schema`, `credential_maintenance_store` and `window_observation` retain ownership. No migration SQL changes.

| Target file | Job and moves | Approx. lines |
| --- | --- | ---: |
| `../sqlite.rs` | Store declarations/errors, async open/read-only/close/schema, narrow shared numeric/error decoding, 526–655 + 687–805 + 4955–4967 + 5853–5894 + 5901–5906; needed constants/imports | 425 |
| `session_records.rs` | ActiveClientCount, event and rollup records with existing impls, 246–501 except insert-only record 253–264 | 280 |
| `account_store.rs` | Async account CRUD/policy list/generation activation, 961–1039 + 1150–1339; account parser 5678–5715 and async invalidation 5742–5796 | 425 |
| `quota_snapshots.rs` | Snapshot write/preserve/read, SQL row decoding/projection, 1040–1149 + 5208–5272 + 5716–5741; SnapshotSelectorProjection | 250 |
| `selector_windows.rs` | Async selector input/exhaustion/window/status/route-state and their validation, 502–525 + 806–960 + 1531–1848 + 4998–5030 + 5060–5099 + 5138–5207 | 730 |
| `affinity_store.rs` | Session-affinity CAS and previous-response ownership SQL/decode, 1340–1530 + 1849–1887 + 5576–5604 + 5626–5677 | 365 |
| `quota_history.rs` | Durable history append/range/purge and parser, 1888–1989 + 5273–5366 | 240 |
| `active_leases.rs` | Acquire/release/count/prune and event-insert record, 255–264 + 1990–2098 + 2395–2571 | 345 |
| `session_history.rs` | Session-event queries, compaction/retention and event decoding, 2099–2179 + 5367–5402 | 165 |
| `session_rollups.rs` | Interval reconstruction, rollup refresh/read/purge/decode and bucket/concurrency math, 2180–2394 + 5403–5575 | 445 |
| `policy_mutation.rs` | Existing weekly-floor/status narrow connection, retry transaction and label resolution, 658–686 + 2572–2890 + 4968–4997; retry constants | 430 |
| `repository_contracts.rs` | Existing async traits and their thin impls, 2892–3094, exported at `sqlite::*` | 240 |
| `sync_accounts.rs` | Fixture-only store opening/account/generation methods and AccountStateRepository impl, 3095–3406 + 4012–4028 + 4724–4741 | 390 |
| `sync_snapshots.rs` | Fixture-only snapshot methods and QuotaSnapshotRepository impl, 3407–3609 + 4742–4763 | 270 |
| `sync_selectors.rs` | Fixture-only selector/status methods/adapters, 3610–4011 + 4764–4820 + 5031–5059 + 5797–5852 | 645 |
| `sync_migrations.rs` | Fixture migration dispatch, v11–v13 rebuild/rollback/schema checks and identifier inspection, 120–151 + 4029–4198 + 4676–4723 + 5895–5899; versioned policy-table literals 94–119 | 340 |
| `legacy_schema_v1to5.rs` | Existing fixture migration versions 1–5, 4199–4454 | 290 |
| `legacy_schema_v6to10.rs` | Existing fixture migration versions 6–10 and auxiliary schema literals, 152–245 + 4455–4675 | 350 |
| `sync_affinity.rs` | Existing sync AffinityRepository impl, 4821–4954 | 170 |
| `selector_windows_tests.rs` | Exact persisted 300-second staleness boundary test, 5100–5137 | 55 |
| `affinity_store_tests.rs` | Exact corrupt-session pin error test, 5605–5625 | 40 |

`session_records` and `repository_contracts` exports are the necessary public module tree. Store type definitions stay in the parent so moved inherent impls access private connection/path fields without crate-wide widening. `session_records` fields read by history/lease/rollup siblings become `pub(super)` only where needed. Already crate-visible `pool`, `read_only`, `sqlx_error`, invalidation and integer conversion helpers remain available to existing credential/window owners from `crate::sqlite`. Put quota parsers with their matching query owner and expose only exact shared decoders to sibling fixture code. The statement at 255–264 moves with active lease writes rather than being duplicated with public event records.

Dependency direction: lifecycle → existing schema/migration owners; account mutation → quota invalidation; snapshots → selector-window insertion; selector input → account/affinity-independent quota rows + existing credential/window readers; active leases → its existing event insertion + session records. History compaction retains its independent DELETE; rollups own interval reconstruction and depend only on session records/numeric conversions. Keeping the interval query with rollup calculation avoids a history↔rollup dependency cycle. Sync fixture methods use the same relevant row parsers while retaining their existing feature fence. Existing async traits simply bind to moved inherent methods; no new repository interface is invented.

Risky seams: SQLx `query!` strings and argument bindings must move byte-for-byte; do not change query strings to make metadata pass. Store open still delegates to the established migration owner, and read-only open never migrates. The floor setter retains its separate pool, busy deadline and retry delays. Sync children are declared and imported under `cfg(any(test, feature = "sync-rusqlite-fixtures"))`; retain individual cfg fences on rusqlite imports because the source guard validates those lines, not Rust's enclosing-module cfg. Existing raw corruption/rollback methods keep `cfg(test)`. Existing database CHECK constraints are moved, never repaired in this refactor.

The widened source collection also reaches the old fully qualified `&rusqlite::Transaction` parameter at 5033, previously beyond the root's first cfg(test) truncation. Resolve such moved type paths through individually cfg-fenced imports, preserving their exact Rust types and the guard's unchanged filter. Do not weaken the filter to compensate for the expanded input.

Baseline inventory: two embedded unit tests, four `sqlx::query!` sites at 970/998/1155/1426 and one `query_scalar!` at 2125. The checked-query strings and bindings remain identical even though their source locations move. No individual method exceeds 900 lines; the oversized units are impl containers.

## 6. Account selection — `crates/codex-router-proxy/src/account_selection.rs`

Current responsibilities: imports/existing floor-switch peer child at 1–89; runtime aliases/provider scope/shared state at 90–229; lease reporter/reservation guard and release at 230–494; hold/exhaustion/queue state at 495–568; decision, selector contracts and errors at 569–780; sync/async repository selector declarations/builders at 781–1073; sync selection at 1074–1191; async projection/affinity/pin release/atomic reservation at 1192–1534; affinity/admission predicates at 1535–1659; candidate filtering/choice/hold/error/wait at 1660–2117; reservations/queue/quarantine mutations at 2118–2343; post-exhaustion assessment at 2344–2539; conversion/request metadata/bounded scanning/hash/clock at 2540–2798; tests/fixtures at 2799–5540.

Retain `src/account_selection.rs` as public declaration/contract owner, with children beneath `src/account_selection/`.

| Target file | One job and moves | Approx. lines |
| --- | --- | ---: |
| `../account_selection.rs` | Existing runtime aliases/shared state, decision/selector/error contracts and selector declarations, 90–229 + 495–568 + 569–817; ActiveReservationGuard declaration 315–320 and existing clocks | 555 |
| `active_reservations.rs` | Existing lease reporter, guard impl/inner Drop and reserve/count/release operations, 230–314 + 321–494 + 1764–1774 + 2118–2217 + 2540–2550; telemetry/process-run code 2775–2792 | 440 |
| `repository_selection.rs` | Existing async builders and complete async selector trait impl, 869–1073 + 1191–1534 | 610 |
| `fixture_selection.rs` | Existing cfg(test) sync repository selector constructors/trait impl and selector-window conversion, 818–868 + 1074–1190 + 1693–1712 + 2090–2107 + 2551–2568 + 2759–2770 | 280 |
| `affinity_admission.rs` | Session/previous-response owner lookup, eligibility/yield and affinity publication, 1535–1659 | 160 |
| `assessment_selection.rs` | Quota selector constructor/trait impl, filtering, strict preferred choice, held account and assessment errors, 757–780 impls + 1660–1692 + 1713–1732 + 1775–1939 + 2069–2089 + 2108–2117 + 2728–2758 + 2771–2774 | 345 |
| `runtime_quarantine.rs` | Runtime exhaustion filtering and queue health mutations, 1733–1763 + 2218–2343 | 195 |
| `short_quota_wait.rs` | Exact fresh-short-reset/jitter policy, 1940–2068 and relevant constants | 165 |
| `post_exhaustion.rs` | Existing read-only alternative projection and safe fresh peer assessment, 2344–2539 | 235 |
| `request_metadata.rs` | Route kind/profile/path and bounded previous_response_id JSON scan, 2569–2727; scan constants | 195 |
| `account_admission.rs` | Existing live read-only peer owner/child tests, retained | 188 |
| `account_selection_tests.rs` | Test coordinator/shared imports, original 2800–2840 | 75 |

The root retains `QuotaAwareAccountSelector`'s declaration while its existing impls move; the row above does not duplicate that impl in the root. Runtime constants stay beside the operation they govern; only constants read by more than one child remain parent-private. The existing `UnixClock` stays a parent alias, not a new clock abstraction. `ActiveReservationGuardInner` is leaf-owned with parent-scoped type visibility for the public guard's private Arc field; no caller gains direct access to its release flag or inner state.

Test suites beneath `src/account_selection/account_selection_tests/`:

| Target file | Existing batch and job | Approx. lines |
| --- | --- | ---: |
| `claude_admission_tests.rs` | Profile/header/pin reuse/release/contention and existing assertion harness, 2840–3205 | 410 |
| `provider_hold_tests.rs` | Provider filtering, reserve yielding and provider-scoped holds, 3237–3369 | 170 |
| `reservation_lease_tests.rs` | Guard re-reservation and external release-clock behavior, 3415–3497 | 125 |
| `selection_authority_tests.rs` | Strict preference, stale authority, attempts and quarantine/queue logs, 3498–3773 | 320 |
| `post_exhaustion_tests.rs` | Queue/unknown/stale/short-wait/weekly stop, 3794–3985 | 230 |
| `selection_concurrency_tests.rs` | Shared projection lock, unowned session race and failed late write, 3986–4268 | 330 |
| `affinity_reconciliation_tests.rs` | Live-activity reconciliation and materially weaker hold, 4269–4420 | 195 |
| `last_resort_tests.rs` | Short guard fallback and post-exhaustion assessment fixture, 4421–4530 | 155 |
| `short_quota_tests.rs` | Reset/jitter/freshness rules, 4537–4631 | 130 |
| `lease_report_fixtures.rs` | Existing RecordingLeaseReporter, 3371–3414 | 65 |
| `projection_read_fixtures.rs` | Panic/Static projection repositories, 4657–4880 | 255 |
| `affinity_race_fixtures.rs` | Existing Slow projection/affinity repository and blocking read, 4881–4900 + 4940–5364 | 475 |
| `affinity_write_fixtures.rs` | Existing controlled failing DB-write actor repository, 4901–4939 | 65 |
| `quota_input_fixtures.rs` | Claude/runtime/post-exhaustion/short-only quota inputs and active-pin predicate, 3206–3237 + 3775–3794 + 4532–4537 + 4632–4656 + 5365–5509 | 265 |
| `lease_wait_fixtures.rs` | Existing database path and bounded active-client wait, 5510–5539 | 50 |

Dependency direction: repository selection → read-only selection projection, affinity admission, assessment selection, runtime quarantine, request metadata and reservations; reservations → existing DbWriteActor/cache/reporting contracts; post-exhaustion and floor peer → same projection/quarantine/queue/lock state, without acquiring a new source of truth. Assessment selection → burn-down and existing weighted selector. Request metadata and quota-wait policy are leaves.

Risky seams: the same `SelectionReservationLock` must continue to serialize projection through reservation and pin publication; preserve its exact scope. Keep std mutex scopes on the synchronous portion, never over newly introduced awaits. Preserve Claude pin CAS contention attempt counts, existing provider-specific cache behavior, short-wait debug env cfg, ActiveReservationGuard Arc/AtomicBool/Drop behavior and post-exhaustion deadline semantics. The existing floor peer imports parent-private runtime fields/functions; retain access within the subtree rather than promoting fields to crate-wide API. Slow/Static/Panic repositories are test assumptions already present, not new stand-ins; their failure behavior and Notify coordination move intact.

Baseline inventory: 40 direct cases (21 plain + 19 Tokio) and one existing test in `account_admission.rs`. The latter stays with the unchanged peer module. The largest individual runtime behavior method is the complete 335-line async selection body, which remains one item.

## Approved source-guard input adaptations

On 2026-10-02 the Lead approved these limited adaptations: collect each existing source root plus all new production children by directory discovery, excluding `_tests.rs`, preserve existing forbidden checks/filters/assertions, truncate each file at its first `#[cfg(test)]`, and additionally fail if the collection is empty or omits the root. No dependency or new glob crate is necessary: use existing standard-library directory traversal. Keep fixture-only file/import cfg fences visible to the unchanged SQLx-only line filter.

| Existing guard | Current source input | Adapted source input |
| --- | --- | --- |
| state/lib.rs:2171 `production_state_storage_does_not_use_rusqlite` | `src/sqlite.rs` via manifest-root read | `src/sqlite.rs` plus production children recursively under `src/sqlite/`; exclude test coordinator/suites/fixtures; exact per-file prefix/filter and forbidden-line assertion retained |
| selection/burn_down.rs:4522 `preferred_next_matches_first_strict_candidate_without_smooth_selector` | Relative `include_str!("burn_down.rs")` | Manifest-root `src/burn_down.rs` plus production children recursively under `src/burn_down/`; exclude test coordinator/suites/fixtures; exact per-file prefix and forbidden WeightedDeficitSelector assertion retained |

Search of all six commissioned files for `include_str!`, `read_to_string`, `source_path` and `src/` found these two source-scanning tests. Other matching proxy reads consume HTTP/audit artifacts, not Rust source, and move unchanged. Selection's separate oracle test uses a JSON fixture include, outside this source-tree adaptation.

Test fixtures reside below the `_tests` coordinator directory; skip that entire test subtree, not just names ending `_tests.rs`, so fixtures do not enter the production guard. Collection includes cfg-gated sync implementation children; feature fencing remains essential. Changes to source collection and new collection-completeness checks are the only approved departures from otherwise unchanged test bodies; imports/relative paths/visibility naturally change with moves.

## Phase 2 preservation and proof contract

Before moves, merge the landed #115/main head and refresh all inventories/test counts. Record a baseline test-list manifest and a source-item inventory from that head. During moves, compare normalized item bodies and SQL literals to the baseline, allowing only imports/module paths/required visibility/source-guard input adaptation. Retain every test by name and attribute; module-qualified test paths can change with the new tree, so compare case identities with the corresponding old/new module mapping rather than raw full strings. Inspect external test filters and update only names invalidated by the move. Verify all resulting moved files against the 900-line budget after formatting, with no aggregate replacement megafile.

One coherent commit per old file/group. Per commit: `cargo fmt --all -- --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`, and affected package tests. Check both default production and fixture-feature builds for the state split. Use the existing real listener/SQLite/concurrent-selection tests for affected runtime proof; source analysis alone proves no runnable behavior.

Final gates retain the repository CI contract: workspace format/clippy; pinned tooling bootstrap; `python3 scripts/tooling/prepare-sqlx.py --check`; CI-profile workspace nextest with `codex-router-cli/keychain-test-support`, including the quota-reset harness and its resource grouping. The exact CI commands live at `.github/workflows/ci.yml:59,93–99,153–161` and `.config/nextest.toml`; host access required for listener/PTY/socket proof must be obtained through the host's permission tool. Report pass/fail counts and exit codes. Do not use user account homes or production Router/Host as proof fixtures. SQLx preparation already creates isolated schema databases from native migration files.

After local proof, push one PR with old → new mapping, current checks and independent move-only review. No merge, tag, release, install or production-process replacement is authorized. Clean workspace debug artifacts only when idle after checking their location and consumers; `target/debug` was absent during Phase 1.

## Review state

This is a bounded module map approved by the commissioning Lead as the Phase 2 basis, not a complete Requirements/Specification/Program Design cycle. Its governing input is the explicit structure-only commission; it does not change behavior, storage ownership, public APIs or domain meaning. The map trades more files and some narrowly widened internal visibility for smaller responsibility owners; the future reader pays navigation cost, while unchanged module contracts keep consumers stable. Lead review requested only descriptive version-range names for the two legacy schema files; the table incorporates that correction.

Phase 1 complete. The six Worker evidence reports are `tmp/module-maps/{proxy-root,websocket,burn-down,state-root,sqlite,account-selection}.md`; the coverage receipt is `tmp/module-maps/map-coverage.json`. These scratch artifacts are ignored by Git; this proposal carries the portable map and counts. Research coverage is recorded in `tmp/practices-research/2026-10-02-split-oversized-files/research-ledger.md`.

| Source | Baseline lines | Direct tests assigned exactly once |
| --- | ---: | ---: |
| proxy/lib.rs | 13,090 | 152 |
| proxy/websocket.rs | 7,188 | 44 |
| selection/burn_down.rs | 6,086 | 74 |
| state/lib.rs | 5,989 | 79 |
| state/sqlite.rs | 5,906 | 2 |
| proxy/account_selection.rs | 5,540 | 40 |
| Total | 43,799 | 391 |

Verification: exact branch/HEAD checked; all six test inventory counts independently matched the full attributed-body range-coverage scan; product/config diff against `dea647b5` exited 0. Proposal check exited 0 for 164 file-budget rows (including retained/coordinator files), maximum 730 lines, two/three-word new module names and whitespace. Existing external child counts are listed with their owning sections and are preserved separately. Target line counts remain estimates until moves and formatting. Actual compilation, tests, SQLx metadata and runtime behavior are intentionally unverified during this analysis-only phase.

Checkpoint: completed map approved by the Lead; schema filenames corrected. No board root/post created. Phase 2 remains gated on #115/main merge and the Lead's merge-commit DM. No source/test/config changes, builds or runtime validation have occurred. Return token: `ready-for-implementation` — this approved document is the Phase 2 basis, with execution explicitly held until its merge prerequisite is fulfilled.
