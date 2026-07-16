# Integrated Quota Reset Implementation Plan

## Goal and terminal

Implement the accepted integrated quota-reset contract inside the existing `codex-router quota`
TUI, prove it without any real provider endpoint or reset consumption, run one implementation
review/remediation cycle, and finish with an open PR proven ready but not merged.

Normative source:

- `docs/specs/2026-07-14-integrated-quota-reset/2026-07-14-integrated-quota-reset.md`

Goal state:

- `tmp/workflow-state/2026-07-15-integrated-quota-reset/details.md`
- `tmp/workflow-state/2026-07-15-integrated-quota-reset/events.jsonl`

## Non-goals and hard safety boundaries

- Do not call a real usage, credit-inventory, refresh, or consume provider endpoint during
  implementation, tests, review, smoke, or PR validation.
- Do not consume a reset credit.
- Do not write SQLite, refresh/write credentials, or create missing secret roots on reads.
- Do not restart, stop, or replace the production Codex router process.
- Do not redesign unrelated CLI commands, the background quota refresher, or general application
  event routing.
- Do not retain compatibility with the standalone picker/render loops.
- Do not merge the PR.

If an automated path can resolve ambient credentials or a non-loopback provider destination, stop
that proof lane and repair composition before continuing. Policy text alone is not proof.

## Current-state anchors

- `crates/codex-router-cli/src/lib.rs:82-143` special-cases the obsolete standalone reset as the
  only native async command.
- `crates/codex-router-cli/src/quota.rs:1266-1375` owns persisted status composition and a blocking
  reload adapter.
- `crates/codex-router-cli/src/presentation/quota.rs:190-210` creates a nested Tokio runtime;
  `:224-480` owns the existing quota loop/shell/focus; `:798-864` is the detail-pane seam.
- `crates/codex-router-cli/src/presentation/quota_reset.rs` owns the standalone picker and second
  confirmation render loop that must be removed.
- `crates/codex-router-cli/src/quota_reset/orchestration.rs` skips credit inspection when weekly is
  ineligible, mints redeem authority during inspection, clones prepared state, and borrows it for
  reusable consume.
- `crates/codex-router-cli/src/quota_reset/provider.rs` accepts arbitrary origins and collects
  unbounded response text.
- `crates/codex-router-cli/src/quota_reset/credentials.rs` already uses async read-only SQLite and
  bounded blocking read-only secret access; extend this seam.
- `.github/workflows/ci.yml:45-62` establishes format, clippy, nextest, deny, audit, and workflow
  lint as authoritative CI gates.

## Target module boundaries

Use the existing session-picker separation pattern where it helps readability. Preserve the single
quota component and account-selection owner while extracting reset-specific responsibilities.

```text
lib.rs
  owns: process dispatch and injected terminal predicate
          |
          v
quota/
  command.rs              owns async QuotaCommand dispatch only
  options.rs              owns quota status/refresh option parsing
  status_command.rs       owns interactive/static status coordination
  status_loader.rs        owns async query-only persisted reads
  status_model.rs         owns status/report/view data types
  status_projection.rs    owns typed display projection
  status_formatting.rs    owns plain/table/JSON formatting
  status_pace.rs          owns burn/run-rate/reset-pace calculations
  status_json.rs          owns JSON DTO projection/serialization
  status_metrics.rs       owns quota status telemetry emission
  refresh_command.rs      owns explicit async refresh coordination
  refresh_service.rs      owns refresh workflow/account decisions
  refresh_provider.rs     owns refresh HTTP protocol
  refresh_history.rs      owns burn-history observation/retention
  background_refresh_worker.rs owns the unchanged serve-owned worker/runtime
  selection_projection.rs owns route-band/runtime-selection projection
  tests/                  owns tests grouped by the same responsibilities
          |
          v
presentation/quota/
  component.rs  owns one iocraft loop, key routing, responsive shell
  model.rs      owns stable AccountId focus and render-safe view state
  render.rs     owns unchanged browse shell/list/detail geometry
  reset.rs      owns reset-detail/confirmation/result rendering only
  test_support.rs / tests.rs own deterministic frames and events
          |
          v
quota_reset/
  domain.rs     pure credit policy and validated inventory
  workflow.rs   pure reducer, intents, effects, correlated outcomes
  credentials.rs read-only account/credential authority + fingerprint
  provider.rs   fixed production HTTP and test-only loopback transport
  service.rs    effect execution, revalidation, single-use capability
  supervisor.rs command-level task ownership and result reduction
  supervisor/effects.rs task outputs, generation allocation, redeem identity minting
  supervisor/protocol.rs render-safe intents, snapshots, and pinned-target protocol
  supervisor/session_state.rs session bookkeeping and snapshot projection
  test_support.rs fake ledgers/held effects behind test composition
```

### Command-session protocol and reducer ownership

`QuotaInteractiveSession` is a workflow/service-layer command session composed by the CLI; it is
not CLI argument dispatch and does not move transitions into `lib.rs`. It is the single
authoritative owner of `ResetWorkflowState`, the pure reducer instance, authority-bearing values,
and all reset task handles.

- Presentation sends render-safe `ResetIntent` values through a bounded intent port.
- The session reduces intents, emits typed effects, and asks non-spawning service methods for one
  operation future at a time.
- The session alone spawns/stores/aborts/awaits those futures and reduces their correlated outcomes.
- The session publishes immutable redacted `ResetWorkflowSnapshot` values through a watch/snapshot
  port. Presentation subscribes and copies snapshots only for rendering; it never runs a second
  reducer or holds reset effect handles/capabilities/auth.
- The quota component may own one snapshot-subscription handler, but no provider/service future or
  reset `JoinHandle`.
- On ordinary precommit render-loop exit or channel close, the session invalidates generations,
  cancels and awaits precommit tasks, then returns.
- After consume invocation, ordinary exit keys are disabled. If the render loop fails or its output
  port closes, the command session remains alive, awaits the committed future to a known/unknown
  result, drops authority, and returns a sanitized command outcome. It never detaches or retries.

`ResetWorkflowService` owns validation/operation logic but never calls `spawn` and never owns task
handles. Structural proof enforces no `spawn`/`JoinHandle` in service modules and no reset task
future in presentation.

Exact filenames may be consolidated only when a module stays below 600 lines and retains one reason
to change. Do not add more behavior to the current 2,314-line `presentation/quota.rs`; split it by
the responsibilities above during the first owning slice. Hard-cut the 5,342-line `quota.rs` into
the named `quota/` responsibilities before reset integration. Avoid generic names such as
`utils.rs`, `helpers.rs`, `common.rs`, `data.rs`, or `process.rs`. Every named responsibility remains
identifiable; consolidation is allowed only when the resulting file is below 600 lines. No finished
quota source file may be 900 lines or larger. The finished quota command family has no blanket
dead-code or lint allowance.

## Execution DAG

```text
Gate 0: verify spec/state, capture browse baseline, record red commands
  |
  v
Slice 1a: stable identity/native async substrate (serialized hotspot owner)
  |
  v
Slice 2a: freeze shared request/outcome/authority contracts
  |
  +-- Slice 2b: finish pure reducer/domain ------------+
  |                                                    |
  +-- Slice 3a -> 3b: GET then consume provider -------+--> Gate 1 foundations
  |                                                    |      integrated
  +-- Slice 4: read-only credential authority --------+
                                                       |
                                                       v
                              Slice 5a: inspection service
                                                       |
                                                       v
                              Slice 5b: revalidation/commit service
                                                       |
                                                       v
                              Slice 1b: command effect supervisor
                                                       |
                                                       v
                              Slice 6a -> 6b -> 6c: integrated quota TUI
                                                       |
                                                       v
                              Slice 7a: legacy hard cutover
                                                       |
                                                       v
                              Slice 7b: dedicated hermetic PTY executable
                                                       |
                                                       v
                         full proof -> implementation review
                                      -> one remediation pass
                                      -> PR wrap-up, unmerged
```

Only Slice 2b, Slice 3, and Slice 4 are parallel, and only after Slice 2a freezes their shared
contracts. Slice 1a owns `lib.rs`, the `quota.rs` to `quota/` cutover, and quota presentation
exclusively. Module exports,
manifests, and shared integration files are parent-owned. Slices 5a–7b are serialized.

## Gate 0 — preflight, harness feasibility, and browse baseline

1. Verify the Worktrunk branch, current HEAD, goal pointers, spec commit, and absence of unrelated
   staged files. Preserve the default-main checkout and its unrelated untracked file.
2. Record current file owners and test commands. Do not read home router state or secrets.
3. Adopt or reject the existing in-progress Slice 2–4 diff before further hotspot work:
   - map every changed hunk/symbol to Slice 2a, 2b, 3a, 3b, or 4;
   - reject obsolete immutable-SQLite and superseded contract code;
   - remove temporary blanket lint allowances and split any finished source file at or above 900
     lines by named responsibility;
   - restore parent ownership of `quota_reset/mod.rs` integration;
   - run the intended slice-local tests, format, clippy, and diff checks;
   - commit accepted scoped foundations as explicit adoption checkpoints before Slice 1a3 work.
   The adoption ledger is parent-owned and must distinguish inherited worker reports from current
   parent verification. No current diff is accepted merely because it compiles.
4. Freeze the hermetic harness contract before feature work:
   - Cargo feature: `quota-reset-test-harness`.
   - Dedicated binary: `codex-router-quota-reset-test-harness` at
     `crates/codex-router-cli/src/bin/quota_reset_test_harness.rs`, with
     `required-features = ["quota-reset-test-harness"]`.
   - Integration test: `crates/codex-router-cli/tests/quota_reset_pty.rs`.
   - Shared entry: a composition-parameterized internal quota-session dispatcher used by production
     and harness wrappers. Generic sealed composition types may compile with the package feature;
     the installed `codex-router` main and production factory must have no reference or runtime route
     to loopback construction. Production behavior always constructs the zero-argument fixed reset
     provider. The harness wrapper can construct only a loopback transport plus isolated roots.
   - Compile-only red/green spike proves default `codex-router` builds, the harness is unavailable
     without its feature, the harness builds with it, and default/all-feature dependency graphs keep
     loopback unreachable from installed parser/production factory. No provider or credential is
     accessed in this spike.
5. Add deterministic normalized browse baselines before changing presentation behavior:
   narrow/48, representative 100x24, 159 stacked, 160 sidecar, clipped short height, resize, empty,
   error, and ordinary exit. Extend fixtures until every case exists; inability to produce one stops
   Slice 1a0.
   - Permanent expected frames: `crates/codex-router-cli/tests/golden/quota/*.txt`.
   - Normalizer: `presentation/quota/test_support.rs`; it may normalize ANSI cursor/color control
     sequences and nondeterministic spinner glyph only, never semantic text, spacing, geometry, or
     account ordering.
   - Explicit update command:
     `UPDATE_QUOTA_GOLDENS=1 cargo test -p codex-router-cli quota_golden -- --nocapture`.
     Review the tracked diff, then prove it without the variable using the same test filter.
   - A deliberate semantic/geometry drift test must fail before accepting the harness.
6. Run and record the baseline relevant CLI test set. A baseline pass is not the red phase for new
   behavior.
7. For each slice, add the smallest failing test first and capture the expected failure before
   implementation. Do not approve new goldens blindly.

Gate evidence: branch/status/HEAD, baseline commands with exit codes/counts, normalized browse
artifacts, and a no-provider/no-home-state statement.

## Slice 1a — quota module extraction, native async, and stable identity

Source: R1–R8, R22–R23 and the interactive dispatch matrix.

Behavior:

- Only effective table status with both stdin and stdout TTY enters interactive quota.
- The existing top-level Tokio runtime owns the complete quota CLI command family. Status in every
  format, refresh, integrated reset, and legacy guidance dispatch through the async quota entry.
  Remove nested runtimes, `block_on`, and OS-thread wrappers from every `QuotaCommand` call graph.
  The serve-owned background refresh worker remains behaviorally unchanged and unreachable from
  quota CLI dispatch.
- All quota-command persisted and provider operations are awaitable. Static/plain/JSON/non-TTY/help paths remain
  noninteractive and construct no reset dependencies; their pure formatting stays synchronous.
- Presentation rows carry hidden `AccountId` and active generation. Focus is stored by `AccountId`;
  index is derived after reload.
- Duplicate labels, reorder, insertion, removal, or generation change cannot retarget focus or an
  attempt.

Write owner and internal checkpoints:

- `crates/codex-router-cli/src/lib.rs` dispatch predicate/composition only.
- `crates/codex-router-cli/src/quota/` named command/status/refresh/projection modules above; the
  old monolithic `quota.rs` is removed after the hard cutover.
- Split `crates/codex-router-cli/src/presentation/quota.rs` into the target quota module structure;
  this slice changes identity/focus and async entry but adds no reset-mode rendering.
- Adjacent/integration tests for dispatch, identity, reload, and browse baselines.

Execute as seven separately proven checkpoints:

- Slice 1a0: behavior-preserving presentation module extraction only. Move code/tests by
  responsibility without changing symbols/logic/output. The complete browse corpus must remain
  identical before any semantic edit.
- Slice 1a1: introduce one injected effective-format × stdin-TTY × stdout-TTY predicate, native
  async entry, and awaitable persisted loader/reload. Prove static paths remain dependency-free.
- Slice 1a2: add hidden AccountId/generation DTOs and semantic focus/reload behavior.
- Slice 1a3a: extraction-only status model/projection/formatting/JSON/pace/metrics/options and their
  responsibility-grouped tests. Preserve output and behavior.
- Slice 1a3b: extraction-only explicit refresh provider/service/history plus the separately named,
  unchanged serve-owned background worker. Preserve refresh and worker behavior.
- Slice 1a3c: convert every remaining `QuotaCommand` path to the single async entry; remove sync
  runtime/thread wrappers from the quota CLI call graph while leaving pure formatting synchronous.
- Slice 1a3d: delete the old monolith/export shim and enforce final module ownership and line limits.

Red/green proof:

- Injected format × stdin TTY × stdout TTY matrix initially fails because ordinary quota is sync.
- Runtime-ownership test initially fails on nested `Runtime`/`block_on`.
- Duplicate-label/reorder/removal focus tests initially fail on row-index focus.
- Green: targeted unit/component tests, scoped call-graph check proving no `QuotaCommand`
  `Runtime`/`block_on`/thread wrapper, strict format/clippy, unchanged normalized browse captures,
  unchanged plain/table/JSON output, refresh/history/provider tests, and unchanged background-worker
  behavior at the owning checkpoints.

Checkpoint after each sub-slice: parent first proves extraction-only browse equality, then one TTY
predicate/loop/runtime, stable focus/reload, status-family extraction, refresh/worker extraction,
quota-command async ownership, and final monolith removal/module limits. Freeze each boundary before
the next; do not mix extraction-only moves with the async cutover.

Split trigger: if async loading requires changes outside quota command ownership, add a narrow
quota-only async source port. Do not introduce a generic event bus or convert unrelated commands.
Quota refresh behavior remains unchanged while its execution and I/O ownership become native async.

## Slice 2 — shared contracts, pure correlated reducer, and validated inventory

Source: R7–R8, R11–R21, R24a and state/effect separation.

Behavior:

- Define render-safe states, intents, independent effects, operation states, correlated request/
  result envelopes, attempt/operation generations, and pure transition reduction.
- Model Browse → Inspecting → Inspected → Confirming → Revalidating → Committing → Result.
- Enforce strict live `<1%`, default No, disabled-Yes non-focus, reset to No on authority loss,
  repeated-key suppression, stale/out-of-order suppression, and exact known/unknown classification.
- Validate the complete safe inventory; deterministic finite-expiry then non-expiring order;
  earliest usable selection; expired/malformed/unknown fail closed; deterministic page/range math.
- Define opaque authority types and a non-Clone/non-Serialize by-value commit capability interface,
  but do not add IO.

Write owner:

- `quota_reset/domain.rs`, new `quota_reset/workflow.rs`, and pure adjacent test modules.
- No iocraft, filesystem, state, secret, reqwest, or raw payload imports.

Red/green proof:

- Exhaustive transition/key table, eligibility matrix, validation/order cases, inventory paging,
  all five semantic operation states, previous/saved provenance, stale correlations, confirmation,
  repeated keys, and commit classification.
- Static/compile contract proving presentation cannot construct/clone/serialize commit authority.

Checkpoint: parent inspects the render-safe boundary and freezes request/outcome/capability
interfaces for slices 3–6.

This slice has two commit-sized checkpoints:

- Slice 2a defines and tests only shared identities, request/outcome envelopes, operation kinds,
  render-safe states, provider-port result categories, and opaque authority ownership. Parent checks
  dependency direction and freezes these interfaces.
- Slice 2b completes reducer transitions, inventory policy, paging, confirmation, provenance, and
  classification against the frozen types. It may run in parallel with Slices 3 and 4.

Split trigger: if raw HTTP DTO parsing enters the pure modules, move conversion into provider;
never leak raw payload or auth types upward.

## Slice 3 — fixed-origin bounded provider protocol

Source: R9–R12, R19–R21, provider composition/bounds, and security context.

Behavior:

- Production construction has no origin parameter and uses only
  `https://chatgpt.com/backend-api`; no CLI/env/config/state override.
- A compile-time test-harness seam accepts validated loopback only and cannot fall back to
  production composition.
- Redirects remain disabled; timeouts bounded; no automatic retry.
- Fully fallible request validation/serialization occurs before consume invocation.
- Stream response bodies through the 1,048,576-byte limit plus one; reject oversized declared
  length before collection.
- GET failures are typed live-fact failures. After consume invocation, only a validated known 2xx
  code is definitive; every other failure is sanitized `OutcomeUnknown`.
- Diagnostics use the spec allowlist and never retain or emit secrets, routing/full IDs, headers,
  raw bodies, provider strings, or unsanitized library errors.

Write owner:

- `quota_reset/provider.rs` or child provider modules and loopback protocol tests.
- CLI manifest/lock only if streaming needs an already-reviewed dependency feature; parent owns
  manifest integration.

Red/green proof:

- Exact GET/GET/POST paths, headers, JSON, redirect refusal/no second connection, no retry.
- Known-code table and every unknown class.
- Declared/chunked/missing/lying length at 1,048,575/1,048,576/1,048,577 bytes.
- Timeout, connection refusal after invocation, close after request bytes, truncation/body-read
  failure, malformed JSON, unknown code, and non-2xx.
- Canary scan across errors/debug/snapshots/transcripts.

Checkpoint: parent verifies fixed production construction, loopback-only test construction, bounded
streaming, conservative outcome mapping, and redaction boundary.

This lane is serial internally:

- Slice 3a implements and proves both GET protocols, validated conversion, redirect refusal,
  streaming limits, and sanitized read-failure categories.
- Slice 3b implements pre-serialized consume requests and proves the post-invocation
  `Known | OutcomeUnknown` matrix and no-retry ledger.

Split trigger: if request construction cannot be separated from invocation, split a prepared
request adapter before service integration rather than weakening zero-POST/unknown semantics.

## Slice 4 — read-only authority and credential fingerprint

Source: R5, R8, R17–R20, R23, R25 and the consume authority chain.

Behavior:

- Read one account by stable ID through ordinary async read-only/query-only SQLite with
  `busy_timeout(0)`; require Enabled and exact active generation. Request no write transaction or
  RESERVED/PENDING/EXCLUSIVE lock and perform no busy-handler retry. If a coherent read transaction
  cannot begin immediately, return a typed refusal. Each fresh transaction observes the latest
  committed state visible when it begins. Normal SQLite WAL/SHM reader coordination is allowed;
  immutable/nolock modes are forbidden against the live database.
- Load exact generation secret through bounded `spawn_blocking` and non-creating read-only secret
  store; no refresh, repair, write, or persistent lease.
- Build an opaque domain-separated/length-framed in-memory provider-effective binding over account,
  generation, token bytes, exact ChatGPT routing ID, and credential expiry.
- Revalidation can compare the full confirmation fingerprint without exposing fingerprint/auth to
  reducer, presentation, Debug, errors, logs, or persistence.

Write owner:

- `quota_reset/credentials.rs`, optional `quota_reset/authority.rs`, isolated fixture tests.
- Existing state/secret crates remain read-only dependencies; any required modification is a
  separate scope-gated split.

Red/green proof:

- Enabled/disabled/missing account, generation change, expired credential, missing routing ID,
  exact account selection, same-generation token/routing/expiry replacement, excluded refresh/
  source-only change, framed-digest ambiguity cases, and redacted Debug.
- An event-driven concurrent-WAL fixture proves: generation A is visible; a writer holds a change
  to B while a reset read promptly returns committed A or a typed busy refusal without retry; the
  writer can commit; and a fresh post-commit read observes B and refuses stale authority. Use bounded
  channel/protocol events, not sleeps or zero-elapsed-time assertions. Inspect connection composition
  for read-only, create-if-missing false, `busy_timeout(0)`, query-only, and absence of immutable.
  Assert no SQL data/schema/application-state mutation by reset. SQLite-owned WAL/SHM coordination
  may change. Preserve a strict recursive secret-root manifest: relative path, type, bytes/hash,
  mode, and symlink target. Missing roots/database stay absent.
- Unique per-run roots, neutral ambient configuration, and parallel-safe fixtures.

Checkpoint: parent verifies current query-only authority, zero busy-handler retry/write transaction,
secret-root byte evidence, and that no presentation module can import authority/fingerprint types.

Split trigger: if query-only SQLite cannot observe current WAL state without waiting on the writer,
stop and propose a narrow read port; never use SQLite immutable mode against the live database.

## Slice 5 — inspection, revalidation, and single-use commit service

Source: R9–R10 and R17–R25.

Write owner:

- Replace `quota_reset/orchestration.rs` with focused `service.rs` and test modules.
- `quota_reset/mod.rs` only through parent-owned export/composition edits.
- Dependencies from Slices 2–4 are read-only. No presentation or CLI dispatch edits.

### Slice 5a — independent inspection service

- Non-spawning `inspect_usage(request)` and `inspect_inventory(request)` operations resolve/use one
  pinned read-only authority bundle and return independent futures regardless of eligibility. The
  command session launches and owns both futures.
- Each result carries all frozen correlation fields and updates only its own activity state.
- The command session invalidates operation generations and aborts/awaits precommit work on
  cancellation; late, duplicate, wrong-account, wrong-generation, wrong-phase, or superseded
  results are ignored by its single reducer.
- Previous results remain explicitly previous and never grant authority.

Red/green: replace the current test that expects ineligible usage to skip credits with a failing
two-GET ledger; add held-future partial completion, cancellation, duplicate/out-of-order, and
focused-account-only cases. Green proof uses fake providers only.

### Slice 5b — revalidation and by-value commit

- Explicit enabled `Yes` triggers fresh account/generation/credential reread, both live GETs, exact
  fingerprint equality, earliest-credit equality, and commit-clock expiry checks.
- Every local check and request serialization failure remains precommit with zero POST.
- Successful revalidation mints one redeem ID and one opaque non-Clone capability.
- `consume(capability)` moves it exactly once, transitions to Committing immediately before provider
  invocation, and returns known or unknown only after invocation.

Red/green: mutation matrix for account disable/removal, generation, same-generation token/routing/
expiry, weekly exact value, selected credit ID/status/title/expiry/order, later-credit-only change,
repeated authorization/effect delivery, local serialization, known outcomes, and every unknown
class. Fake ledger proves zero or one POST for every case.

Checkpoint: parent reviews authority lifetimes and the only POST call graph, then runs reducer,
fake-provider, loopback, fixture-byte, redaction, and capability-ownership proof together.

Split trigger: if service state or task ownership depends on iocraft hooks, stop before presentation
integration and define a command-owned effect port.

## Slice 1b — command-session effect supervision and presentation port

Source: R22–R25.

Behavior:

- Implement the `QuotaInteractiveSession` protocol frozen above: it owns the sole reducer, interprets
  effects, owns task handles and authority, receives intents, and publishes redacted snapshots.
- Read-only/precommit handles are cancellable and invalidated on mode teardown.
- Once a capability is consumed, ownership transfers to the supervisor; resize, mode changes,
  component teardown, and ordinary cancel keys neither abort nor detach it.
- Await a typed known result or bounded unknown outcome, reduce only correlated results, then drop
  auth/fingerprint/full IDs without persistence.
- No DB connection, transaction, mutex, credential lease, or blocking task spans terminal
  confirmation or provider IO.

Write owner:

- New `quota_reset/session.rs` (or `supervisor.rs`), typed intent/snapshot ports, service integration
  tests, and parent-owned module export.
- A narrow quota-component snapshot subscription/intent sender may be compiled in a test component,
  but no reset rendering or shell geometry edits occur yet.

Red/green proof: structural tests show service has no spawn/JoinHandle and presentation owns no reset
future/handle. Deterministic channel/task probes cover GET teardown, channel close, cancellation
races, POST invocation boundary, render-loop/component teardown during held POST, command future
remaining pending after render exit, bounded outcome, final command completion, and authority drop.
No sleeps; use protocol notifications and bounded waits.

Checkpoint: parent proves consumptive tasks are neither hook-owned nor detached before the TUI may
drive them.

## Slice 6 — integrated quota detail mode

Source: Product decisions 1–7, 9, 11–13; R1–R18 and R24–R25.

Write owner:

- `presentation/quota/{component,model,render,reset,test_support,tests}.rs` after Slice 1a split.
- Parent-owned `quota/`/`lib.rs` composition touchpoints only at integration checkpoints.
- Presentation imports only render-safe workflow state, intents, and effect handles—never secrets,
  fingerprints, commit capabilities, provider clients, reqwest, or raw payloads.

### Slice 6a — shell bridge and pinned focus

- Route `Ctrl-R` from the existing focused stable AccountId into one reset attempt.
- Preserve the exact title, route line, list renderer, bars, spacing, clipping, height budgets,
  responsive thresholds, navigation, and terminal lifecycle.
- Persisted reload retains the pinned reset target and pane or invalidates safely when account/
  generation disappears; it never retargets.
- Returning restores ordinary detail for the same surviving ID.

Red/green: Ctrl-R intent, duplicate labels, reorder/insertion/removal/generation, clipped focus, and
browse-reset-browse frames. Browse differential must remain green before proceeding.

### Slice 6b — reset detail, confirmation, inventory, and results

- Render five named semantic activity rows; loading/succeeded/failed/cancelled/dispatched meaning is
  visible without color or animation.
- Render current/previous/saved provenance and count disagreement without confusing authority.
- Render complete validated inventory, earliest highlight, finite/non-expiring expiry, PgUp/PgDn
  range/remaining count, and no silent clipping.
- Implement inspected Enter, default No, disabled Yes non-focus/revert-to-No, explicit enabled Yes,
  revalidating, committing, known results, refusal, and outcome unknown.
- Every result/refusal/unknown state states that persisted browse data may remain stale until normal
  quota refresh. Returning to browse performs no provider refresh and no persistence.
- Dynamic shortcuts match each mode; no messages appear below the TUI.

Red/green: normalized frames for every spec state at narrow, 100x24, 159, 160, wide/sidecar,
clipped inventory, partial results, previous-refreshing, disagreement, ineligible/eligible
confirmation, revalidation, committing, all known results, precommit failure, unknown outcome, and
the stale-browse notice. A bomb refresh/persistence port and ledger prove returning from result
performs neither operation.

### Slice 6c — held-future responsiveness and round trip

- Hold and independently release both inspection GETs, both revalidation GETs, and consume POST.
- While each waits, prove input, resize, spinner, persisted reload, cancel rules, stale suppression,
  and semantic activity frames.
- Prove browse → reset → browse exact shell restoration and zero POST on cancellation.

Checkpoint: parent compares normalized browse baseline to final browse output, reviews the full
shared hotspot diff, and runs deterministic component/fake-ledger proof. Any title/list/geometry
change blocks cutover.

Split trigger: if reset rendering requires changing shell/list geometry helpers instead of injecting
only detail/footer, stop and repair the presentation boundary.

## Slice 7a — hard legacy cutover and structural enforcement

Source: Product decisions 1, 8, 10; R2–R4 and legacy compatibility.

- Remove the reset-only native async special case, standalone workflow entry, picker, confirmation
  loop, and their obsolete tests. Delete `presentation/quota_reset.rs` after integrated tests pass;
  retain no unreachable compatibility path.
- Bare `codex-router quota reset` writes exactly the specified guidance plus newline to stdout,
  nothing to stderr, exit 0, for any TTY combination, and constructs no state/secret/provider/reset
  dependency.
- `quota reset --help`, `-h`, and `help` use the existing zero-exit help path with migration copy.
  Every other flag/positional argument remains the ordinary parser error with exit 2. There is no
  reset `--router-root` option.
- Add structural tests: one render loop/account-selection owner; no nested interactive runtime; no
  secret/raw HTTP/fingerprint/commit authority imports in presentation; no row/width/iocraft in
  workflow; only by-value capability reaches POST. Scope the reset-origin assertion specifically to
  `quota_reset` and production reset composition: production calls a zero-argument fixed-origin
  constructor and no string/URL/origin parameter reaches it. Preserve existing configurable quota
  refresh behavior and its tests unchanged.

Write owner: serialized edits to `lib.rs`, `quota/`, `presentation/mod.rs`, deletion of standalone
presentation, reset composition/module exports, exact CLI/architecture tests.

Red/green: exact parser/process-independent IO matrix and bomb factories/sentinel paths prove zero
dependency construction before deleting the old paths. Final `rg`/architecture tests prove hard
cutover.

Checkpoint: parent verifies installed command behavior, help, structural source graph, and zero
network/state/credential ledger.

## Slice 7b — hermetic compiled PTY executable

Source: Proof expectations 6, 8, and 9; R1–R4, R22, R24–R25.

Do not add a loopback override to the installed `codex-router` CLI. Complete the Gate-0 scaffold and
implement its contract:
feature `quota-reset-test-harness`, binary `codex-router-quota-reset-test-harness`, and integration
test `quota_reset_pty`. The harness calls the same composition-parameterized async interactive quota
dispatcher, session, reducer, supervisor, presentation component, and iocraft entry while receiving
an already-bound loopback transport and isolated roots through a sealed test composition. Generic
composition types may compile in an all-feature library build, but
the installed `codex-router` main, parser, and production factory have no reference, option,
environment path, or runtime route to loopback construction. Architecture/call-graph tests prove
unreachability in default and all-feature target graphs.

The dedicated harness binary owns a small parser separate from the installed CLI parser. Its only
authority-bearing inputs are explicit isolated fixture paths and the bare HTTP origin derived from
the listener already bound by the parent test on `127.0.0.1:0` or `[::1]:0`. It rejects missing,
relative, ambient-home, non-loopback, userinfo, path, query, fragment, and unassigned-port inputs
before opening state or credentials. It never consults environment for roots or provider routing.
After validation, production and harness wrappers converge on one composition-parameterized async
interactive quota dispatcher; the harness supplies the loopback provider and isolated authority
reader while the installed wrapper supplies the zero-argument fixed provider and normal explicit
router root. Neither wrapper duplicates parser-to-session, session-to-supervisor, or iocraft-loop
behavior.

Use a permanent Rust PTY integration driver (prefer a narrowly scoped dev dependency such as
`portable-pty` only after license/deny/API validation). The driver:

1. Creates unique temporary state/secret roots with fixture-only canary credentials. It snapshots a
   strict recursive secret-root manifest: relative entry set, file type, bytes/hash, mode, and
   symlink target where permitted. The state fixture proves current committed WAL visibility, zero
   busy waiting/writer locks, and no SQL data/schema/application-state mutation; SQLite-owned
   WAL/SHM coordination may change. Missing roots/database remain absent.
2. Binds port 0 on loopback and passes only that already-bound listener's bare origin to the sealed
   harness parser; the provider constructor revalidates loopback and an egress guard rejects all
   other destinations before credential lookup/request construction.
3. Starts the dedicated compiled executable in a PTY with ambient HOME/router/provider variables
   neutralized.
4. Observes browse, focuses a fixture account, sends `Ctrl-R`, observes both inspection activities,
   exercises resize, returns with `Ctrl-R`/Esc before confirmation, verifies zero POST, exits, and
   proves terminal restoration/no output below the TUI.
5. Uses bounded semantic/protocol waits, reaps child/listener/tasks on all paths, and never prints
   canary secrets or raw request material.

Inject timeout, semantic-wait failure, early listener failure, and assertion/early-return paths.
Each cleanup test proves bounded child termination and wait/reap, listener closure and port reuse,
task abort/join, temporary-root cleanup, and no leaked canary output.

The red phase proves the PTY harness boots and then fails because integrated Ctrl-R behavior is
absent. Green proof uses these literal commands:

```text
cargo metadata --no-deps --format-version 1
cargo build -p codex-router-cli --bin codex-router
cargo build -p codex-router-cli --features quota-reset-test-harness --bin codex-router-quota-reset-test-harness
cargo clippy -p codex-router-cli --features quota-reset-test-harness --bin codex-router-quota-reset-test-harness --test quota_reset_pty -- -D warnings
cargo nextest run -p codex-router-cli --features quota-reset-test-harness --test quota_reset_pty
```

The parent owns `crates/codex-router-cli/Cargo.toml`, `Cargo.lock`, and `.github/workflows/ci.yml` for
this integration. Add a named CI step that executes the feature-specific build, clippy, and nextest
commands above; actionlint and final-SHA GitHub logs must prove the step ran. Default workspace CI
remaining green does not satisfy the PTY gate. Shell static smoke is not a substitute.

Split trigger: if test-only loopback selection becomes reachable from the installed binary or the
harness requires ambient home/provider state, stop and rework composition; do not weaken the gate.

## Requirements/proof matrix

All behavioral rows require observed red then green. “Final” means rerun after implementation-review
remediation against the final pushed SHA. Evidence sources are parent-run unless explicitly marked
GitHub.

| Requirement | Owner | Proof modality/layer | Evidence source | Freshness guard | Split trigger |
| --- | --- | --- | --- | --- | --- |
| R1 | 1a,6a–c | component + normalized visual + PTY | browse differential and round trip | baseline vs final HEAD at narrow/159/160/wide | split shell bridge from reset render on geometry drift |
| R2 | 1a,7a,7b | component + structural + PTY | loop/owner counters and source graph | final source and compiled executable | split async entry from integration |
| R3 | 6a–c,7b | component + visual + PTY | footer/key frames and terminal transcript | final text/frame, bounded waits | isolate PTY output proof if mock cannot see trailing output |
| R4 | 1a,7a | unit + CLI integration | format/TTY matrix with bomb dependencies | final parser/dispatcher | split predicate from static writer regression |
| R5 | 1a,4 | unit + component + structural | hidden DTO identity and redacted output | final DTO/render state | split projection from presentation |
| R6 | 1a,6a | reducer/component | duplicate label, reorder, insert, remove | final reload path | split semantic focus reducer |
| R7 | 2a–b | unit/component | correlation mismatch table | deterministic IDs, final reducer | split envelope types before effects |
| R8 | 1a,6a | component + fixture integration | pinned pane and invalidation cases | unique fixture, final component | split reload from invalidation |
| R9 | 5a | fake-provider integration | exactly two GETs for focused account only | fresh ledger, loopback-only | split targeting from execution |
| R10 | 2b,5a,6b | unit + held fake + visual | both GETs, partial success/error, disabled Yes | current attempt/op generations | split activity model from render |
| R11 | 2b,6b | unit + component + visual | validation/order, highlight, paging/range | fixed clock/final viewport | split inventory policy from scrolling |
| R12 | 2b,3a | unit + loopback | expiry/status/id/timestamp/malformed table | fixed clock/raw fixtures local | split decoding from usability |
| R13 | 2b,6b | unit + visual | saved/live labels, warning, live-only choice | final labels; saved types cannot build authority | split provenance model |
| R14 | 2b,6b | exhaustive unit + visual | only fresh 0% + usable credit enables | final reducer/current ops | split eligibility from reason rendering |
| R15 | 2b,6b | component + visual + canary | exact confirmation safe fields/warning | randomized canaries/final frames | split confirmation model/render |
| R16 | 2b,5a,6b | unit + component + ledger | Enter/no-op/default No/repeated keys | final event mapping | split keys from service effects |
| R16a | 2b,6b | unit + component + ledger + visual | disabled non-focus/revert to No/zero POST | current authority only | split focus policy from invalidation race |
| R17 | 4,5b | unit + fixture + fake/loopback | full mutation/fingerprint/revalidation matrix | unique roots/fixed clock/no ambient auth | split fingerprint, reread, live revalidation |
| R18 | 2b,5b | parameterized unit/integration | every precommit terminal path zero POST | empty ledger per case/final wording | split cancellation from refusals |
| R19 | 2a,5b | type/structural + integration | non-Clone/non-Serialize, by-value one POST | final API/unique attempt | split capability type from consume |
| R20 | 3b,5b | unit + fake ledger + loopback + visual | prep zero POST; post-invoke unknown/no retry | fault recorded after invocation | split request preparation from adapter |
| R21 | 3b,6b | parser/loopback + visual | four known codes vs every unknown class | final enum/1 MiB bound | split protocol from outcome render |
| R22 | 1a,7b | structural + integration + PTY | all QuotaCommand variants under top runtime; no command block_on/thread wrapper; background worker unreachable | final compiled executable + rooted call graph | mandatory async substrate slice |
| R23 | 1a,4 | integration + structural/lints | awaitable command I/O; read-only/query-only busy_timeout(0); no immutable; no held authority | final task graph + event-driven WAL fixture | split loader from secret boundary |
| R24 | 5a,1b,6c,7b | held-future component/integration/PTy | keys/resize/reload/cancel/stale while held | bounded notifications, no sleeps | split precommit cancel/postcommit supervision |
| R24a | 2b,6b–c | state table + normalized visual | five operations/all semantic states | frozen animation/final text | split semantic model from capture matrix |
| R25 | 1b,5b,6c,7b | task probes + structural + PTY | teardown cancels GET, not committed POST | bounded outcome/final command owner | replan supervisor if component owns commit |
| Product 1 standalone removed | 7a | structural + CLI/component | source graph and one-owner tests | final source | stop on reachable old path |
| Product 2 focused Ctrl-R | 1a2,6a | component | stable-ID key/reload cases | final component | split focus from workflow |
| Product 3 pre-POST back | 2b,5a,6b | reducer + ledger + visual | Esc/Ctrl-R zero POST | final reducer/ledger | split cancellation race |
| Product 4 independent reads | 5a,6b | held fake + component | both GETs/partial frames | current attempt/op | split effect/render |
| Product 5 live authority | 2b,5b | unit + ledger | saved/live disagreement and live-only selection | final types | stop if saved builds authority |
| Product 6 explicit Yes | 2b,6b | reducer + component + ledger | default No/enabled Yes/Enter | final event mapping | split selection/effect |
| Product 7 post-invoke wait | 1b,5b,6c | task probes + visual | held POST/known-or-unknown | final session owner | replan on hook ownership |
| Product 8 migration guidance | 7a | parser/process IO | exact bare/help/error matrix | final binary/parser | split parser from run |
| Product 9 no result refresh/write | 5b,6b | bomb ports + ledger + visual | stale-browse notice and zero side effects | final result transitions | stop on implicit refresh |
| Product 10 no real proof | 3,4,7b | egress/fixture/architecture | loopback-only and ambient bombs | every automated run | stop on reachable real origin |
| Product 11 conditional Yes | 2b,6b | exhaustive unit + visual | fresh <1% + usable credit only | final reducer | split reason rendering |
| Product 12 visible operations | 2b,6b–c | state table + frames | all five semantic rows | final text/frozen spinner | split activity model |
| Product 13 full inventory | 2b,6b | unit + component + visual | complete paging/range/expiry/status | fixed clock/final viewport | split policy/scrolling |
| Product 14 invocation boundary | 3b,5b | fake + loopback | preparation zero POST/postinvoke unknown | recorded boundary | split prepare/invoke |
| Product 15 credential binding | 4,5b | fixture mutation matrix | token/routing/expiry equality | unique roots/fixed clock | split binding/reread |
| Product 16 single-use capability | 2a,5b | type/structural + ledger | non-Clone by-value exactly one POST | final API | split type/consume |
| CLI contract | 1a,7a | parser/IO/process tests | exact bare/help/error/stdout/stderr/exit/dependency ledger | final binary/parser | split parsing from dependency-free run |
| Provider/security | 3a–b | loopback + structural + canary | origin, redirect, timeout, size ±1, diagnostics | loopback-only test composition/final provider | split transport, diagnostics, production composition |
| Read-only/no-real-provider | 4,7b | concurrent WAL + secret bytes + egress architecture + PTY | latest committed state per transaction, no write transaction/busy retry/immutable mode, unique roots/ports, neutral env, zero ambient path | each run fresh/fail closed | stop immediately if ambient/non-loopback reachable |
| Architecture/cutover | 7a | structural + clippy | owner/import/call-graph assertions | final source graph | split module cutover from enforcement |
| Terminal end-to-end | 7b | compiled process PTY + visual/protocol | focus/Ctrl-R/activity/resize/back/zero POST/restore | final built target/bounded waits | split driver/server but recombine terminal proof |
| Quality/CI | terminal | fmt, clippy, nextest, build, deny, audit, actionlint | command receipts + GitHub checks | after final remediation commit | scope guard on unrelated failure |
| Implementation review | terminal | one review/remediation cycle | report, disposition, affected reruns | final implementation diff | material finding returns once to executor |
| PR readiness | terminal | GitHub checks/threads/mergeability | PR URL and fresh final-SHA queries | after final push; watch interval 45 | external failure remains open; never merge |

## Validation command gates

Each slice records its exact fully qualified test filter in the handoff before the red run. The
authoritative gate selectors are fixed here:

1. Slice-local red/green: `cargo test -p codex-router-cli <fully-qualified-test-filter> -- --nocapture`;
   replace only the filter with the recorded permanent test name. The expected red reason is
   captured before code.
2. Targeted CLI package: `cargo nextest run -p codex-router-cli`.
3. Harness metadata/build/clippy/test: the five literal Slice 7b commands, including
   `cargo nextest run -p codex-router-cli --features quota-reset-test-harness --test quota_reset_pty`.
4. Format: `cargo fmt --all -- --check`.
5. Clippy: `cargo clippy --workspace --all-targets -- -D warnings` plus the literal harness clippy.
6. Full tests: `cargo nextest run --workspace` plus the literal harness nextest command.
7. Build: `cargo build --workspace` plus the literal installed and harness binary builds.
8. Supply chain: `cargo deny check`; `cargo audit`.
9. CI: Rust, harness PTY, and Workflow lint checks, then one implementation-review/remediation cycle.
10. PR: blocking `gh pr checks <pr> --watch --interval 45`, then fresh comments, unresolved threads,
   mergeability, head SHA, and unmerged state.

Do not run live/manual provider smoke. “Manual UX” proof uses the hermetic PTY executable and
loopback fixtures only.

## Checkpoints, commits, and handoff evidence

Commit at each parent-verified capability boundary; never stage unrelated files:

1. async quota identity/browse substrate;
2. reducer/contracts;
3. bounded provider;
4. read-only authority;
5. inspection/revalidation/one-shot service;
6. command supervisor;
7. integrated TUI;
8. hard cutover/hermetic PTY;
9. implementation-review remediation when needed.

Every slice handoff includes touched files, requirement IDs, red and green commands/output/counts,
fake/loopback ledgers, fixture byte evidence, captures/transcripts, no-real-provider attestation,
risks, interface assumptions, and verified commit SHA.

## Rollback and recovery

- No data migration exists. Precommit failure invalidates generations, cancels/awaits read-only
  tasks, closes read-only DB, drops authority, and leaves saved browse state unchanged.
- After invocation, never retry/reconcile/claim rollback; show known or unknown only.
- Tests use RAII unique temp roots/listeners/children and reap/close them even on timeout. Retain no
  secret-bearing failure artifact.
- Implementation rollback is a scoped commit revert, not a compatibility dual path.

## Open questions

None. Gate 0 fixes the session ownership protocol, harness target/feature/API boundary, and literal
commands before feature work. The only implementation-time selection is the PTY dev dependency;
validate its API/license with a compile-only spike. If it cannot satisfy the frozen boundary, stop
at Gate 0 rather than exposing a production override or weakening terminal proof.

## Phase footer

phase_result: complete
evidence: this plan; accepted spec; tmp/plan-workflows/2026-07-15-integrated-quota-reset/reviews/combined-delta-review-and-remediation.md
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: The single authorized combined spec/plan delta review and one remediation pass are parent-verified; every requirement has executable ownership, proof, commands, and a final PR-ready gate.
