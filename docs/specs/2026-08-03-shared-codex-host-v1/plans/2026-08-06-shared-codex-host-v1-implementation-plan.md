# Shared Codex Host V1 Responsibility Refactor Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use
> `superpowers:subagent-driven-development` or `superpowers:executing-plans`
> with `superpowers:test-driven-development`. Steps use checkbox (`- [ ]`)
> syntax for tracking.

**Goal:** Refactor the already-working Shared Codex Host implementation into
responsibility-named Rust modules without changing runtime behavior, public CLI
behavior, process topology, protocol traffic, results, or error classifications.

**Architecture:** Keep the current one foreground host and one Tokio lifecycle
owner task. Clients continue to attach directly to the upstream app-server Unix
socket. Split only mixed policy aggregates; closely coupled values and errors
remain with their behavioral owner, and thin Rust namespace façades contain no
policy.

**Tech Stack:** Rust 2024, Tokio async process/net/signal/time/sync,
tokio-tungstenite, serde, rustix, existing iocraft/indicatif presentation, and
the existing test/runtime fixtures. Shared Host adds no persistence or SQLx.

## Global constraints

- The adjacent August Requirements, Specification, and Program Design are the
  only authoritative design inputs; July artifacts are historical trials.
- Preserve current HEAD behavior: no upstream Codex changes, client proxy,
  persistence, launchd, polling, adoption, multi-generation handoff, rollback,
  fleet control, second lifecycle task, or new runtime hop.
- Keep one lifecycle owner task with retained child handles and in-memory state.
- Use Tokio async I/O, process, socket, signal, timer, and bounded channel APIs;
  no blocking wait, private runtime, unbounded queue, or lock across `.await`.
- Keep terminal libraries in `codex-router-cli`; host and Codex adapter crates
  return typed values and have no iocraft/indicatif dependency.
- Do not turn each type into its own file. A module groups cohesive values,
  errors, and helpers that share one behavioral reason to change.
- Generic policy buckets (`host.rs`, `runtime.rs`, `domain.rs`, `process.rs`,
  `operator_protocol.rs`, or a policy-bearing `protocol.rs`) do not survive.
  `mod.rs` and crate roots may remain as declarations and narrow re-exports.
- Existing unrelated large CLI, session, presentation, quota, and test-support
  files are out of scope.
- Never stop, restart, signal, or replace the production router without the
  exact user authorization required by repository instructions.
- Stop at PR-ready and unmerged. After implementation, obtain independent
  Claude Opus 5 and GPT-5.6-Sol high reviews and perform exactly one bounded
  remediation cycle.

---

## Target source map

The names below are responsibility promises, not one-file-per-component
machinery. During execution, a proposed move must be collapsed when it contains
only pass-through code.

### `codex-router-cli`

- `host_command/mod.rs`: parse/dispatch only.
- `host_command/foreground_launch.rs`: resolve roots and compose typed host
  launch inputs.
- `host_command/operator_client.rs`: one bounded operator exchange.
- `host_command/update_outcome.rs`: EOF/reconnect/`await-host-start` outcome.
- `presentation/host.rs`: retain the current host presenter; do not split unless
  source evidence shows two independent presentation reasons to change.

### `codex-router-codex`

- Keep focused `paths.rs`, `profile.rs`, `executable.rs`, and `session.rs`.
- Rename/split mixed app-server integration into
  `app_server_launch.rs`, `app_server_control_protocol.rs`, and
  `remote_control_observation.rs`.
- The initialized experimental WebSocket remains one exchange; do not add a
  public daemon probe or second observation connection.

### `codex-router-host`

- `host_configuration.rs`: immutable validated inputs only.
- `host_singleton_authority.rs`: lock, stale socket, inherited authority, and
  retained listener.
- `operator_messages.rs`: requests, progress/terminal envelopes,
  classifications, codecs, and serialization of lifecycle-owned snapshots.
- `operator_connection.rs`: bounded accepted-stream I/O and backpressure.
- `process_group_child.rs`: Tokio child retention and exact child/group signals.
- `router_compatibility.rs`: static health observation and classification.
- `owned_router.rs`: retained router child start/stop behavior only.
- `managed_app_server.rs`: retained app-server child and spawn identity.
- `app_server_endpoint_guard.rs`: fail-closed foreign endpoint exclusion.
- `app_server_shutdown.rs`: expected-exit identity, one-signal invariant,
  escalation, retained progress, and shutdown classification.
- `explicit_app_server_restart.rs`: app-server stop/guard/start/readiness and
  recovery-budget reset result.
- `explicit_router_restart.rs`: owned-only router stop/start/readiness result.
- `codex_update_preparation.rs`: identity-before, official updater containment,
  identity-after, and pre-activation classification.
- `changed_update_activation.rs`: post-change teardown, telemetry boundary, and
  same-process exec preparation.
- `lifecycle_state.rs`: lifecycle dimensions, snapshot fields/invariants, and
  hosted-readiness derivation—the sole semantic snapshot owner.
- `lifecycle_owner.rs` plus focused owner-operation children: the one select
  loop and bounded startup/admission/completion/recovery/status/shutdown
  operations. Children are functions/modules, never new owner tasks.
- `lifecycle_telemetry.rs`: redacted observations and pre-exec flush boundary.

### Tests

- Partition Shared Host tests by invariant: singleton/operator transport,
  lifecycle state, app-server shutdown, explicit restart, update/re-exec, direct
  attachment, and presentation.
- Reuse current permanent fixtures. Do not create another umbrella Shared Host
  fixture or duplicate upstream protocol behavior.

---

### Task 1: Lock the behavioral and dependency baseline

**Requirements:** R1-R10, V1-V10; behavior-preservation gate.

**Files:**

- Modify: this plan only when observed commands differ from repository tooling.
- Test: existing workspace suites; no product source edit.

- [ ] **Step 1: Record the exact baseline**

  Record HEAD, `git status --short`, changed files, and hashes of the three
  authoritative artifacts. Preserve the untracked Mindle settings file and do
  not stage it.

- [ ] **Step 2: Run focused pre-refactor behavior tests**

  Run:

  ```bash
  cargo nextest run -p codex-router-codex -p codex-router-host -p codex-router-cli
  ```

  Expected: exit 0. Record pass counts and any pre-existing unrelated failure
  separately; do not change unrelated code to make this gate green.

- [ ] **Step 3: Run structural red assertions**

  Run an explicit source inventory asserting that policy still exists in the
  generic files named in Global constraints. Expected: FAIL because current
  HEAD still contains those mixed aggregates. This is the refactor red gate;
  behavioral tests must remain green.

---

### Task 2: Refactor the version-bounded Codex adapter

**Requirements:** R2-R5, R7, R10; V2-V6, V9-V10.

**Files:**

- Rename/split: `crates/codex-router-codex/src/app_server.rs`
- Rename/split: `crates/codex-router-codex/src/protocol.rs`
- Modify: `crates/codex-router-codex/src/lib.rs`
- Test: existing adapter unit/protocol/session tests.

**Produces:** existing public launch, initialization/version, Remote Control,
executable, profile, paths, and session interfaces with unchanged semantics.

- [ ] **Step 1: Add or relocate structural ownership assertions**

  Assert launch argv tests exercise the launch-owned public API, protocol
  framing tests exercise the control-protocol-owned public API, and Remote
  Control convergence tests exercise the observation-owned public API. Verify
  the old mixed module inventory fails; integration tests need not import
  private module paths.

- [ ] **Step 2: Move code without changing interfaces or protocol ordering**

  Preserve `initialize`/`initialized`, running-version extraction, and
  `remoteControl/status/read` on the same initialized WebSocket. Keep local
  result/error types beside their owner and expose only required crate exports.

- [ ] **Step 3: Prove the slice**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p codex-router-codex --all-targets -- -D warnings
  cargo nextest run -p codex-router-codex
  ```

  Expected: all exit 0; current adapter test count does not decrease.

---

### Task 3: Refactor host contracts and retained child owners

**Requirements:** R1, R4, R6, R8-R10; C1; F1-F2, F5-F7; V1, V3-V5, V7-V10.

**Files:**

- Rename/split current host `config.rs`, `domain.rs`, `instance.rs`,
  `operator_protocol.rs`, `process.rs`, `router.rs`, `app_server.rs`, and
  `restart.rs` into the target source map.
- Modify: `crates/codex-router-host/src/lib.rs`
- Split only touched tests by invariant.

**Interfaces:** Preserve current public request/response, configuration,
snapshot, child, shutdown, and restart types unless a visibility reduction is
compiler-proved safe across all workspace consumers.

- [ ] **Step 1: Establish lifecycle-state ownership tests**

  Ensure lifecycle-state tests prove snapshot fields/invariants and
  `hosted_readiness`; operator-message tests prove only envelope,
  classification, codec, and serialization behavior. The same snapshot type is
  reused—no DTO or conversion layer is added.

- [ ] **Step 2: Move contract and singleton boundaries**

  Move code mechanically first, keeping tests green after each owner. Move
  `send_operator_request` and `OperatorClientError` to the CLI Operator Client;
  keep message codecs public only as required by the CLI and black-box tests.
  Lifecycle admission does not move into connection handling. Host integration
  tests use a test-local bounded client over the public message contract rather
  than preserving the production client in the host crate.

- [ ] **Step 3: Move child ownership and restart policy**

  Preserve exact-child/process-group signalling, one-signal shutdown progress,
  endpoint exclusion, external-router no-signal behavior, and recovery-budget
  reset at native readiness. Do not add task, mutex, queue, retry, or adoption
  abstractions.

- [ ] **Step 4: Prove the slice**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p codex-router-host --all-targets -- -D warnings
  cargo nextest run -p codex-router-host
  cargo tree -p codex-router-host --edges normal
  ```

  Expected: all exit 0; dependency tree still excludes CLI, UI, state, SQLx,
  auth, quota, and proxy crates.

---

### Task 4: Refactor the lifecycle owner, update path, and CLI composition

**Requirements:** R1-R3, R6-R10; C1-C3; F1-F7; V1-V10.

**Files:**

- Rename/split: `crates/codex-router-host/src/runtime.rs` and current
  `runtime/*` helpers into `lifecycle_owner.rs`, `lifecycle_state.rs`, and
  responsibility-named owner-operation modules.
- Rename/narrow: current host `update.rs` and `telemetry.rs`.
- Rename/split: `crates/codex-router-cli/src/host.rs` into `host_command/*`.
- Modify crate/module façades and only directly affected tests.

- [ ] **Step 1: Pin the one-owner invariant before moves**

  Existing concurrency/recovery/update tests must prove one mutation owner,
  bounded transport admission, status during mutation, caller disconnect
  semantics, one recovery attempt, and stop arbitration. Do not create a new
  behavioral test unless an existing invariant lacks coverage.

- [ ] **Step 2: Move lifecycle policies behind one owner task**

  Keep one `tokio::select!` owner loop and retained handles in that task. Owner
  operations accept bounded inputs and return typed results; they do not spawn
  lifecycle authorities or share mutable lifecycle state.

- [ ] **Step 3: Move update preparation and activation**

  Preserve the exact four outcomes, no child signal before changed identity,
  teardown ordering, telemetry flush, inherited singleton authority, exec, and
  caller reconnect/`await-host-start` behavior.

- [ ] **Step 4: Move CLI composition and client observation**

  Keep parsing/dispatch, foreground input composition, bounded operator I/O,
  and cross-exec outcome observation separate. Preserve direct session
  attachment and the existing host presenter; do not add a TUI.

- [ ] **Step 5: Prove the integrated refactor**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p codex-router-host -p codex-router-cli --all-targets -- -D warnings
  cargo nextest run -p codex-router-host -p codex-router-cli
  ```

  Expected: all exit 0; no scoped test count decreases.

---

### Task 5: Structural and real-runtime acceptance

**Requirements:** V1-V10 and Program Design structural constraints.

**Files:**

- Modify tests only for uncovered accepted invariants.
- Modify README only if current README already owns the affected command docs.

- [ ] **Step 1: Turn the structural red gate green**

  Verify generic policy files are gone or façade-only, every target module has
  a stated owner/reason to change, no pass-through module survives, no touched
  Shared Host source unit exceeds 900 lines, and any touched unit near 600 lines
  received an explicit responsibility review.

- [ ] **Step 2: Run full automated proof**

  ```bash
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo nextest run --workspace
  cargo deny check
  git diff --check
  ```

  Record exit codes and pass counts. Required tests may be repartitioned but not
  deleted, ignored, weakened, or replaced by mocks.

- [ ] **Step 3: Run safe manual proof**

  Use isolated debug router state, a fixture/isolated Codex home, and a
  non-production port. Demonstrate help, foreground launch, status, direct
  client attachment, app-server restart, router restart, one recovery and
  exhaustion, no-change/failed update preserving the current process, changed
  update re-exec/reconnect against the fixture install, and bounded foreground
  stop. Observe the existing OTel/Victoria path when available. Run the exact
  installed-version CLI, Desktop native-attachment, and Remote Control gates
  when their external prerequisites are available; otherwise record the exact
  external blocker without adding a fallback. Do not touch the production
  router process.

---

### Task 6: Independent review, one remediation, and PR readiness

**Requirements:** user delivery gate.

- [ ] **Step 1: Freeze review evidence**

  Record exact HEAD/diff, artifact hashes, structural receipt, automated proof,
  and manual proof. Review receipts remain under ignored `tmp/`.

- [ ] **Step 2: Dispatch independent implementation reviews**

  Use Claude Opus 5 and GPT-5.6-Sol high with no shared conversation history.
  Give each the authoritative artifacts, full current diff, proof, non-goals,
  and request requirement → source → concrete failure evidence. Do not silently
  substitute models.

- [ ] **Step 3: Perform exactly one bounded remediation cycle**

  Parent-validate findings. For each accepted behavioral defect, add a failing
  regression test before the smallest in-owner fix. Correct structural defects
  with the structural red/green gate. Do not begin a second remediation cycle;
  unresolved valid findings remain PR blockers.

- [ ] **Step 4: Rerun final proof and prepare the PR**

  Rerun affected and full gates, intentionally stage only scoped files, commit,
  push, update PR #22, inspect checks/comments/threads/mergeability/published
  diff, and stop ready but unmerged. Never stage the Mindle settings file.

---

## Requirement and proof coverage

| Obligation | Tasks | Primary proof |
| --- | --- | --- |
| U1-U10, R1-R10 | 1-5 | unchanged focused/full suites plus manual runtime matrix |
| C1-C3 | 3-5 | singleton/owner tests, direct attachment, process and socket correlation |
| F1-F7 | 3-5 | failure/restart/update/stop invariant tests and runtime proof |
| V1-V10 | 1-5 | dependency, protocol, lifecycle, update, CLI, OTel, and acceptance evidence |
| Review and PR-ready/unmerged | 6 | exact-model receipts, one remediation, CI/thread/mergeability inspection |

## Self-review receipt

- The plan implements no new product behavior; it restructures current HEAD.
- Every generic current aggregate maps to a named owner and proof gate.
- Snapshot semantics have one owner; operator messages only serialize them.
- No second app-server observation connection, lifecycle task, persistence,
  SQLx, proxy, daemon manager, polling, adoption, or handoff system appears.
- Tasks exclude unrelated CLI/session cleanup and avoid one-file-per-type
  splitting.
- Exactly one post-implementation remediation cycle and an unmerged PR are the
  terminal delivery boundary.
