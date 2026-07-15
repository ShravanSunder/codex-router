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
quota.rs + quota/interactive.rs
  owns: persisted projection, async reload, static/interactive composition
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
  test_support.rs fake ledgers/held effects behind test composition
```

Exact filenames may be consolidated when a module stays below 600 lines and retains one reason to
change. Do not add more behavior to the current 2,314-line `presentation/quota.rs`; split it by the
responsibilities above during the first owning slice. Do not move unrelated quota-report logic out
of the 5,342-line `quota.rs` merely to clean it up.

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
contracts. Slice 1a owns `lib.rs`, `quota.rs`, and quota presentation exclusively. Module exports,
manifests, and shared integration files are parent-owned. Slices 5a–7b are serialized.

## Gate 0 — preflight and browse baseline

1. Verify the Worktrunk branch, current HEAD, goal pointers, spec commit, and absence of unrelated
   staged files. Preserve the default-main checkout and its unrelated untracked file.
2. Record current file owners and test commands. Do not read home router state or secrets.
3. Add deterministic normalized browse baselines before changing presentation behavior:
   narrow/48, representative 100x24, 159 stacked, 160 sidecar, clipped short height, resize, empty,
   error, and ordinary exit where current fixtures support them.
4. Run and record the baseline relevant CLI test set. A baseline pass is not the red phase for new
   behavior.
5. For each slice, add the smallest failing test first and capture the expected failure before
   implementation. Do not approve new goldens blindly.

Gate evidence: branch/status/HEAD, baseline commands with exit codes/counts, normalized browse
artifacts, and a no-provider/no-home-state statement.

## Slice 1a — stable identity and native async quota substrate

Source: R1–R8, R22–R23 and the interactive dispatch matrix.

Behavior:

- Only effective table status with both stdin and stdout TTY enters interactive quota.
- The existing top-level Tokio runtime directly awaits one iocraft loop; remove nested runtime and
  `block_on` from the interactive quota call graph.
- Persisted load/reload is awaitable. Static/plain/JSON/non-TTY/help/refresh paths remain existing
  noninteractive writers and construct no reset dependencies.
- Presentation rows carry hidden `AccountId` and active generation. Focus is stored by `AccountId`;
  index is derived after reload.
- Duplicate labels, reorder, insertion, removal, or generation change cannot retarget focus or an
  attempt.

Write owner:

- `crates/codex-router-cli/src/lib.rs` dispatch predicate/composition only.
- `crates/codex-router-cli/src/quota.rs` async interactive source/projection only; optional
  `crates/codex-router-cli/src/quota/interactive.rs`.
- Split `crates/codex-router-cli/src/presentation/quota.rs` into the target quota module structure;
  this slice changes identity/focus and async entry but adds no reset-mode rendering.
- Adjacent/integration tests for dispatch, identity, reload, and browse baselines.

Red/green proof:

- Injected format × stdin TTY × stdout TTY matrix initially fails because ordinary quota is sync.
- Runtime-ownership test initially fails on nested `Runtime`/`block_on`.
- Duplicate-label/reorder/removal focus tests initially fail on row-index focus.
- Green: targeted unit/component tests, scoped structural check, and unchanged normalized browse
  captures at all baseline boundaries.

Checkpoint: parent inspects the hotspot diff, proves one TTY predicate/loop/runtime, and freezes the
identity-bearing presentation/async-loader interfaces before reset integration.

Split trigger: if async loading requires changes outside quota command ownership, add a narrow
quota-only async source port. Do not introduce a generic event bus or convert unrelated commands.

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

- Read one account by stable ID through async query-only SQLite; require Enabled and exact active
  generation.
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
- Before/after SQLite bytes and secret-tree entries/bytes/modes; missing roots are not created.
- Unique per-run roots, neutral ambient configuration, and parallel-safe fixtures.

Checkpoint: parent verifies read-only byte evidence and that no presentation module can import the
authority/fingerprint types.

Split trigger: if existing read-only APIs create metadata or cannot target one account, stop and
propose a narrow read-only port change; do not open writable state or relax byte proof.

## Slice 5 — inspection, revalidation, and single-use commit service

Source: R9–R10 and R17–R25.

Write owner:

- Replace `quota_reset/orchestration.rs` with focused `service.rs` and test modules.
- `quota_reset/mod.rs` only through parent-owned export/composition edits.
- Dependencies from Slices 2–4 are read-only. No presentation or CLI dispatch edits.

### Slice 5a — independent inspection service

- `inspect(request)` resolves one pinned read-only authority bundle and launches usage and inventory
  GETs independently regardless of eligibility.
- Each result carries all frozen correlation fields and updates only its own activity state.
- Cancellation invalidates operation generations and aborts/awaits precommit work; late, duplicate,
  wrong-account, wrong-generation, wrong-phase, or superseded results are ignored.
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

## Slice 1b — command-level effect supervisor

Source: R22–R25.

Behavior:

- Add the command-level owner that interprets reducer effects and owns task handles plus all
  authority-bearing values.
- Read-only/precommit handles are cancellable and invalidated on mode teardown.
- Once a capability is consumed, ownership transfers to the supervisor; resize, mode changes,
  component teardown, and ordinary cancel keys neither abort nor detach it.
- Await a typed known result or bounded unknown outcome, reduce only correlated results, then drop
  auth/fingerprint/full IDs without persistence.
- No DB connection, transaction, mutex, credential lease, or blocking task spans terminal
  confirmation or provider IO.

Write owner:

- New `quota_reset/supervisor.rs`, service integration tests, and parent-owned module export.
- No TUI rendering or shell geometry edits.

Red/green proof: deterministic task probes and drop sentinels for GET teardown, cancellation races,
POST invocation boundary, component/mode teardown during held POST, bounded unknown outcome, and
authority drop. No sleeps; use protocol notifications and bounded waits.

Checkpoint: parent proves consumptive tasks are neither hook-owned nor detached before the TUI may
drive them.

## Slice 6 — integrated quota detail mode

Source: Product decisions 1–7, 9, 11–13; R1–R18 and R24–R25.

Write owner:

- `presentation/quota/{component,model,render,reset,test_support,tests}.rs` after Slice 1a split.
- Parent-owned `quota.rs`/`lib.rs` composition touchpoints only at integration checkpoints.
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
- Dynamic shortcuts match each mode; no messages appear below the TUI.

Red/green: normalized frames for every spec state at narrow, 100x24, 159, 160, wide/sidecar,
clipped inventory, partial results, previous-refreshing, disagreement, ineligible/eligible
confirmation, revalidation, committing, all known results, precommit failure, and unknown outcome.

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
  workflow; no production origin override; only by-value capability reaches POST.

Write owner: serialized edits to `lib.rs`, `quota.rs`, `presentation/mod.rs`, deletion of standalone
presentation, reset composition/module exports, exact CLI/architecture tests.

Red/green: exact parser/process-independent IO matrix and bomb factories/sentinel paths prove zero
dependency construction before deleting the old paths. Final `rg`/architecture tests prove hard
cutover.

Checkpoint: parent verifies installed command behavior, help, structural source graph, and zero
network/state/credential ledger.

## Slice 7b — hermetic compiled PTY executable

Source: Proof expectations 6, 8, and 9; R1–R4, R22, R24–R25.

Do not add a loopback override to the installed `codex-router` CLI, even behind a user-selectable
feature. Add a dedicated test-only executable target with a distinct name and required test-harness
feature. It must call the same parser, async quota composition, reducer, supervisor, and iocraft
entry as the installed CLI while receiving an already-bound loopback transport and isolated roots
through a test-only Rust composition API. The installed binary must not compile or reference that
composition. Architecture tests prove the separation.

Use a permanent Rust PTY integration driver (prefer a narrowly scoped dev dependency such as
`portable-pty` only after license/deny/API validation). The driver:

1. Creates unique temporary state/secret roots with fixture-only canary credentials and snapshots
   database/secret bytes and modes.
2. Binds port 0 on loopback and passes the already-bound test transport; an egress guard rejects all
   other destinations before credential lookup/request construction.
3. Starts the dedicated compiled executable in a PTY with ambient HOME/router/provider variables
   neutralized.
4. Observes browse, focuses a fixture account, sends `Ctrl-R`, observes both inspection activities,
   exercises resize, returns with `Ctrl-R`/Esc before confirmation, verifies zero POST, exits, and
   proves terminal restoration/no output below the TUI.
5. Uses bounded semantic/protocol waits, reaps child/listener/tasks on all paths, and never prints
   canary secrets or raw request material.

The red phase proves the PTY harness boots and then fails because integrated Ctrl-R behavior is
absent. Green proof runs in a named nextest/CI lane that builds the dedicated target. Shell static
smoke is not a substitute.

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
| R22 | 1a,7b | structural + integration + PTY | top runtime, no block_on/nested panic | final compiled executable | mandatory async substrate slice |
| R23 | 1a,4 | integration + structural/lints | awaitable reload/spawn_blocking/no held authority | final task graph; await-holding lints | split loader from secret boundary |
| R24 | 5a,1b,6c,7b | held-future component/integration/PTy | keys/resize/reload/cancel/stale while held | bounded notifications, no sleeps | split precommit cancel/postcommit supervision |
| R24a | 2b,6b–c | state table + normalized visual | five operations/all semantic states | frozen animation/final text | split semantic model from capture matrix |
| R25 | 1b,5b,6c,7b | task probes + structural + PTY | teardown cancels GET, not committed POST | bounded outcome/final command owner | replan supervisor if component owns commit |
| Product 1–16 | all | traceability audit + full-flow frames | matrix plus final transcript | final HEAD | stop if any decision lacks executable row |
| CLI contract | 1a,7a | parser/IO/process tests | exact bare/help/error/stdout/stderr/exit/dependency ledger | final binary/parser | split parsing from dependency-free run |
| Provider/security | 3a–b | loopback + structural + canary | origin, redirect, timeout, size ±1, diagnostics | loopback-only test composition/final provider | split transport, diagnostics, production composition |
| Read-only/no-real-provider | 4,7b | fixture bytes + egress architecture + PTY | unique roots/ports, neutral env, zero ambient path | each run fresh/fail closed | stop immediately if ambient/non-loopback reachable |
| Architecture/cutover | 7a | structural + clippy | owner/import/call-graph assertions | final source graph | split module cutover from enforcement |
| Terminal end-to-end | 7b | compiled process PTY + visual/protocol | focus/Ctrl-R/activity/resize/back/zero POST/restore | final built target/bounded waits | split driver/server but recombine terminal proof |
| Quality/CI | terminal | fmt, clippy, nextest, build, deny, audit, actionlint | command receipts + GitHub checks | after final remediation commit | scope guard on unrelated failure |
| Implementation review | terminal | one review/remediation cycle | report, disposition, affected reruns | final implementation diff | material finding returns once to executor |
| PR readiness | terminal | GitHub checks/threads/mergeability | PR URL and fresh final-SHA queries | after final push; watch interval 45 | external failure remains open; never merge |

## Validation command gates

The executor confirms exact test filters after modules land; authoritative layers are:

1. Slice-local red/green: `cargo test -p codex-router-cli <new-filter> -- --nocapture` or the
   equivalent nextest expression, with the expected red reason recorded before code.
2. Targeted CLI package: `cargo nextest run -p codex-router-cli` plus the named PTY harness feature/
   target command that cannot affect installed composition.
3. Format: `cargo fmt --all -- --check`.
4. Clippy: `cargo clippy --workspace --all-targets -- -D warnings` and include the test-harness target
   in a separate feature-specific clippy command if default all-targets does not build it.
5. Full tests: `cargo nextest run --workspace` plus the named feature-specific PTY lane.
6. Build: `cargo build --workspace` and build the dedicated PTY target in its test-only lane.
7. Supply chain: `cargo deny check`; `cargo audit`.
8. CI: Rust and Workflow lint checks, then one implementation-review/remediation cycle.
9. PR: blocking `gh pr checks <pr> --watch --interval 45`, then fresh comments, unresolved threads,
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

None at product or architecture level. During implementation, validate the dedicated PTY library/
target API and license before manifest changes. If no test-only executable can share the real CLI
dispatch path while remaining structurally unavailable to the installed binary, stop at Slice 7b
and return to the parent rather than exposing a production override.

## Phase footer

phase_result: complete
evidence: this plan; accepted spec; tmp/plan-workflows/2026-07-15-integrated-quota-reset/plan-ledger.md
recommended_next_workflow: shravan-dev-workflow:plan-review-swarm
recommended_transition_reason: Every requirement is assigned to a proof-sized slice, dependency-safe execution order, local red/green evidence, and a final PR-ready gate.
