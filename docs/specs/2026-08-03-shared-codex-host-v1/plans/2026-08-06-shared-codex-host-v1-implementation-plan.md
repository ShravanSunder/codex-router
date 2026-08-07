# Shared Codex Host V1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `shravan-dev-workflow:implementation-execute-plan` and
> `superpowers:test-driven-development` to execute this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the accepted personal-use `codex-router host` foreground
runtime, direct native app-server attachment, bounded recovery, explicit
lifecycle commands, conditional Codex update/re-exec, status, and safe
observability without changing upstream Codex or introducing a client proxy.

**Architecture:** The existing CLI remains the composition and presentation
owner. New `codex-router-codex` and `codex-router-host` library crates isolate
the version-bounded upstream adapter from lifecycle policy. One Tokio owner
task serializes operator commands and owns child handles; clients continue to
connect directly to Codex's conventional Unix socket.

**Tech Stack:** Rust 2024, Tokio async process/net/signal/time/sync, Hyper,
tokio-tungstenite, serde/serde_json, sha2, rustix safe Unix APIs, iocraft and
indicatif in CLI presentation only, existing OpenTelemetry providers.

## Global Constraints

- Authoritative artifacts are the adjacent User Requirements, Specification,
  and Program Design dated 2026-08-03.
- Target one trusted owner on Unix/macOS; no public ingress or reduced-trust
  client tier.
- No upstream Codex change or fork, custom Codex protocol, host traffic proxy,
  launchd, automatic update polling, multi-generation overlap, adoption,
  rollback, persistent lifecycle state, SQLite/SQLx host dependency, polling
  loop, fleet system, or cross-Mac control plane.
- `codex-router serve`, `codex-router sessions`, and `codex-router host` retain
  distinct meanings. Sessions and Desktop attach directly to the conventional
  Codex app-server socket.
- Host code is native async: Tokio process/socket/signal/timer/channel APIs,
  bounded queues, no private runtime, no blocking wait, no unbounded task or
  queue, and no lock/guard across `.await`.
- Workspace Rust lints remain authoritative, including `unsafe_code =
  "forbid"`; process/FD operations use safe standard-library or `rustix` APIs.
- Lifecycle state, deadlines, retries, cancellation, and terminal outcomes stay
  in `codex-router-host`. Presentation models/renderers stay in
  `codex-router-cli::presentation`.
- Interactive layouts, if needed, use iocraft `View`/`Text` layout primitives.
  Bounded non-fullscreen progress may use indicatif. Non-TTY output remains
  deterministic and contains no cursor control.
- Never stop, restart, or replace the production Codex router process during
  development proof unless the owner explicitly says: `replace the production
  Codex router process`.
- A completed implementation receives independent Claude Opus 5 and
  GPT-5.6-Sol high reviews, exactly one bounded remediation cycle, rerun proof,
  and a ready but unmerged PR.

---

## File and ownership map

### New `codex-router-codex` crate

- `src/lib.rs`: public adapter exports only.
- `src/profile.rs`: `CodexRouterProfile` rendering and root override argv.
- `src/paths.rs`: normal Codex-home, conventional socket, and managed binary
  projections.
- `src/executable.rs`: bounded managed executable resolution/version/content
  identity operations.
- `src/app_server.rs`: app-server and updater command specifications.
- `src/protocol.rs`: bounded native websocket initialize/version/Remote Control
  observation.
- `src/session.rs`: root interactive `--remote unix://...` argv projection.

### New `codex-router-host` crate

- `src/lib.rs`: stable public host entry points and domain exports.
- `src/domain.rs`: orthogonal lifecycle/status/result types.
- `src/config.rs`: validated, already-resolved host paths/endpoints/deadlines.
- `src/operator_protocol.rs`: bounded, versioned one-request-per-connection
  protocol.
- `src/instance.rs`: singleton lock, stale socket ownership, inherited-lock
  bootstrap.
- `src/process.rs`: safe Tokio child/process-group/signal primitives.
- `src/router.rs`: compatible external or retained owned-router boundary.
- `src/app_server.rs`: retained child, readiness, exact-child shutdown progress.
- `src/update.rs`: same-path updater/identity/re-exec coordination.
- `src/runtime.rs`: one owner task and event loop.
- `src/telemetry.rs`: low-cardinality host lifecycle instruments/events.
- `tests/operator_runtime.rs`: real Unix socket/singleton/serialization tests.
- `tests/process_lifecycle.rs`: real fixture process groups and signal tests.
- `tests/update_reexec.rs`: updater matrix, FD exclusion, and re-exec tests.

### Existing crates

- `codex-router-core/src/router_compatibility.rs`: static `/healthz` schema.
- `codex-router-proxy/src/server.rs`: unauthenticated static health response
  before auth/upstream/database request work.
- `codex-router-cli/src/host.rs`: host parsing, resolved configuration, async
  dispatch, and operator client.
- `codex-router-cli/src/presentation/host.rs`: typed deterministic renderers and
  optional bounded progress adapter.
- `codex-router-cli/src/telemetry.rs`: explicit bounded pre-exec provider
  shutdown handle.
- `codex-router-cli/src/sessions.rs`: same managed executable and native remote
  projection for new/resume launches.
- `codex-router-test-support/src/shared_host.rs`: permanent child/updater/socket
  fixtures and redacted runtime proof helpers.

---

### Task 1: Add the static router compatibility contract

**Requirements:** R4, V3.

**Files:**

- Create: `crates/codex-router-core/src/router_compatibility.rs`
- Modify: `crates/codex-router-core/src/lib.rs`
- Modify: `crates/codex-router-proxy/src/server.rs`
- Modify: `crates/codex-router-proxy/src/lib.rs`

**Interfaces:**

- Produces:
  `RouterCompatibility::current(local_model_authentication_required: bool)`
  and `ROUTER_COMPATIBILITY_REVISION: u16 = 1`.
- JSON fields are exactly `product`, `compatibility_revision`, `binary_version`,
  and `local_model_authentication_required`.

- [x] **Step 1: Write failing schema and route tests**

```rust
#[test]
fn router_compatibility_contains_only_static_contract_fields()
-> Result<(), serde_json::Error> {
    let value = serde_json::to_value(RouterCompatibility::current(false))?;
    assert_eq!(value["product"], "codex-router");
    assert_eq!(value["compatibility_revision"], 1);
    assert_eq!(value["local_model_authentication_required"], false);
    assert_eq!(value.as_object().map(serde_json::Map::len), Some(4));
    Ok(())
}

#[tokio::test]
async fn healthz_bypasses_local_auth_and_performs_no_upstream_work()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = HealthRouteFixture::start(/* local auth required */ true).await?;
    let response = fixture.get_unauthenticated("/healthz").await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<RouterCompatibility>().await?,
        RouterCompatibility::current(true));
    assert_eq!(fixture.recorded_upstream_request_count(), 0);
    assert_eq!(fixture.state_read_count(), 0);
    Ok(())
}
```

`HealthRouteFixture` is a test-only helper adjacent to the server tests. It
starts the existing loopback runtime on a kernel-assigned port with recording
upstream/state seams and exposes only the four methods used above.

- [x] **Step 2: Run the focused tests and observe the expected missing-contract failures**

Run: `cargo nextest run -p codex-router-core -p codex-router-proxy router_compatibility healthz`

Expected: FAIL because the schema and route do not exist.

- [x] **Step 3: Implement the typed schema and early static response**

```rust
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouterCompatibility {
    pub product: String,
    pub compatibility_revision: u16,
    pub binary_version: String,
    pub local_model_authentication_required: bool,
}
```

Capture the auth-required boolean in `LoopbackProtocolConnectionHandler` at
runtime construction. In `handle_hyper_request`, return the serialized schema
for exactly `GET /healthz` before websocket classification, local auth,
database access, body collection, or upstream selection. Other methods on the
path remain unsupported.

- [x] **Step 4: Run focused tests and formatting**

Run: `cargo fmt --all -- --check`

Run: `cargo nextest run -p codex-router-core -p codex-router-proxy router_compatibility healthz`

Expected: PASS.

- [x] **Step 5: Commit the independently reviewable compatibility slice**

```bash
git add crates/codex-router-core crates/codex-router-proxy
git commit -m "feat: expose router compatibility health"
```

---

### Task 2: Extract the version-bounded Codex adapter

**Requirements:** R2–R5, R7–R10; V2–V4, V6, V9, V10.

**Files:**

- Create: `crates/codex-router-codex/Cargo.toml`
- Create: `crates/codex-router-codex/src/{lib,profile,paths,executable,app_server,protocol,session}.rs`
- Modify: `Cargo.toml`
- Modify: `crates/codex-router-cli/src/profile.rs`
- Modify: `crates/codex-router-cli/Cargo.toml`
- Modify: `crates/codex-router-test-support/Cargo.toml`
- Modify: `crates/codex-router-test-support/src/installed_codex.rs`

**Interfaces:**

```rust
pub struct CodexPaths {
    pub codex_home: PathBuf,
    pub app_server_socket: PathBuf,
    pub managed_executable: PathBuf,
}

pub struct ExecutableIdentity {
    pub canonical_path: PathBuf,
    pub version: String,
    digest: [u8; 32],
}

pub struct AppServerCommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
}

pub enum RemoteControlObservation {
    Connected,
    Connecting,
    Errored,
    Disabled,
    Unavailable { message: String },
}

pub struct AppServerObservation {
    pub running_version: String,
    pub remote_control: RemoteControlObservation,
}
```

- Consumes normal Codex home resolved by CLI. It never reads router root.
- Produces the exact paths
  `$CODEX_HOME/app-server-control/app-server-control.sock` and
  `$CODEX_HOME/packages/standalone/current/codex` on Unix.
- App-server argv is the profile root overrides followed by
  `app-server --remote-control --listen unix://`.
- Session argv inserts `--remote unix://` at the root before `resume`.

- [x] **Step 1: Write failing projection and protocol fixture tests**

```rust
#[test]
fn app_server_spec_uses_one_router_projection_and_native_socket() {
    let profile = CodexRouterProfile::new(8787);
    let spec = AppServerCommandSpec::new(managed_path(), &profile);
    assert_eq!(spec.executable, managed_path());
    assert!(spec.args.ends_with(&os_args([
        "app-server", "--remote-control", "--listen", "unix://",
    ])));
    assert_eq!(profile.root_overrides(), expected_router_overrides());
}

#[tokio::test]
async fn status_probe_initializes_experimental_api_and_maps_status_change()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = NativeProtocolFixture::start(RemoteScenario::ConnectingThenConnected).await?;
    let observation = observe_app_server(fixture.socket_path()).await?;
    assert_eq!(observation.remote_control, RemoteControlObservation::Connected);
    assert_eq!(fixture.received_methods(), [
        "initialize",
        "initialized",
        "remoteControl/status/read",
    ]);
    assert!(fixture.initialize_enabled_experimental_api());
    Ok(())
}
```

`NativeProtocolFixture` and `RemoteScenario` are test-only protocol fixtures in
`protocol.rs`; they bind a temporary Unix socket, upgrade exactly one websocket,
record decoded method names and initialize capabilities, and emit only the
scenario's declared response/notification sequence.

Also add malformed/oversized frame, disabled, errored, timeout, wrong request
id, version mismatch, hash timeout/drain, and session-new/session-resume argv
tests.

- [x] **Step 2: Run and observe missing-crate/test failures**

Run: `cargo nextest run -p codex-router-codex`

Expected: FAIL because the crate is not yet registered.

- [x] **Step 3: Implement focused adapter modules**

Use `tokio::fs::canonicalize`, a capacity-one single-flight identity operation,
and bounded `spawn_blocking` chunked SHA-256 hashing. Use
`tokio::net::UnixStream` plus `tokio_tungstenite::client_async` for native
websocket JSON-RPC. Frame size is capped at 64 KiB; each probe is capped at two
seconds; only a `connecting` status waits for the pinned ten-second changed
notification.

The adapter invokes the same resolved executable path for:

```text
<managed-codex> --version
<managed-codex> <root-overrides...> app-server --remote-control --listen unix://
<managed-codex> app-server daemon version
<managed-codex> update
```

Move `CodexRouterProfile` and its existing renderer into this crate; re-export
it from `codex-router-cli::profile` so existing observable CLI behavior remains
unchanged while test-support imports the new crate directly.

- [x] **Step 4: Record the pinned upstream source-verification receipt**

This is external-contract evidence rather than a product test. Against the
checkout named by `CODEX_ROUTER_PINNED_CODEX_CHECKOUT`, require commit
`2b5bdcf67547860f2e5c5a605009a70026796b2b`, inspect the exact named upstream
symbols, and store the command/output receipt under ignored `tmp/`:

```bash
git -C "$CODEX_ROUTER_PINNED_CODEX_CHECKOUT" rev-parse HEAD
rg -n 'STOP_GRACE_PERIOD|STOP_TIMEOUT|command_args' \
  "$CODEX_ROUTER_PINNED_CODEX_CHECKOUT/codex-rs/app-server-daemon/src/backend/pid.rs"
rg -n 'app_server_control_socket_path|APP_SERVER_CONTROL_SOCKET' \
  "$CODEX_ROUTER_PINNED_CODEX_CHECKOUT/codex-rs/app-server-transport/src/transport/mod.rs"
rg -n 'remoteControl/status/read|remoteControl/status/changed|RemoteControlConnectionStatus' \
  "$CODEX_ROUTER_PINNED_CODEX_CHECKOUT/codex-rs/app-server-protocol/src/protocol"
rg -n 'InteractiveRemoteOptions|Subcommand::Resume|Subcommand::Update' \
  "$CODEX_ROUTER_PINNED_CODEX_CHECKOUT/codex-rs/cli/src/main.rs"
```

The receipt records observed file/line anchors and the semantic conclusion for
argv, socket, Remote Control methods/status variants, root `--remote`, updater,
and 60/70-second shutdown behavior. It does not make the router test suite
assert another repository's private source layout.

- [x] **Step 5: Run adapter, CLI profile, and installed-Codex support tests**

Run: `cargo nextest run -p codex-router-codex -p codex-router-cli -p codex-router-test-support profile session`

Expected: PASS.

- [x] **Step 6: Commit the adapter extraction**

```bash
git add Cargo.toml Cargo.lock crates/codex-router-codex crates/codex-router-cli/src/profile.rs crates/codex-router-cli/Cargo.toml crates/codex-router-test-support
git commit -m "feat: isolate managed Codex integration"
```

---

### Task 3: Define host domain, operator protocol, and singleton authority

**Requirements:** R1, R6, R9, R10; C1; V1, V5, V8, V10.

**Files:**

- Create: `crates/codex-router-host/Cargo.toml`
- Create: `crates/codex-router-host/src/{lib,domain,config,operator_protocol,instance}.rs`
- Create: `crates/codex-router-host/tests/operator_runtime.rs`
- Modify: `Cargo.toml`

**Interfaces:**

```rust
pub enum HostOperation { Start, Status, RestartAppServer, UpdateCodex, RestartRouter }
pub enum HostPhase { Starting, Steady, Mutating { operation: HostOperation, phase: String }, Stopping }
pub enum RecoveryBudget { Available, Consumed }
pub enum HostedReadiness { Ready, LocalReadyRemoteDegraded, Unavailable }
pub struct HostSnapshot { /* exact typed dimensions from Program Design */ }
pub enum OperatorRequest { Status, AwaitHostStart, RestartAppServer, UpdateCodex, RestartRouter }
pub enum OperatorFrame { Progress(HostProgress), Terminal(HostTerminalResponse) }
```

`HostConfig` receives resolved paths and endpoints; it never reads environment
variables. The operator protocol uses newline-delimited JSON with a 64 KiB
maximum frame and exactly one request per connection.

- [x] **Step 1: Write failing domain/protocol/authority tests**

Cover JSON version mismatch, oversized/multiple requests, busy classification,
orthogonal snapshot derivation, owner-only socket mode, contender-never-unlinks,
stale socket removal only after lock acquisition, and debug/installed/explicit
root isolation.

- [x] **Step 2: Run the failing host tests**

Run: `cargo nextest run -p codex-router-host operator singleton protocol`

Expected: FAIL because the crate and interfaces do not exist.

- [x] **Step 3: Implement safe singleton ownership**

Use `std::fs::File::try_lock` for the inert stable artifact. Preserve authority
through changed-update exec with safe `rustix` only:

```rust
rustix::stdio::dup2_stdin(&lock_file)?;
rustix::io::fcntl_setfd(rustix::stdio::stdin(), FdFlags::empty())?;
// exec
// replacement validates fstat against configured lock artifact, then:
rustix::io::fcntl_setfd(rustix::stdio::stdin(), FdFlags::CLOEXEC)?;
```

The inherited-lock bootstrap is accepted only with a private same-version
environment marker and exact metadata match. Ordinary start acquires the lock
normally. Lock/socket artifacts contain no PID or lifecycle state.

- [x] **Step 4: Run host authority tests and dependency inspection**

Run: `cargo nextest run -p codex-router-host operator singleton protocol`

Run: `cargo tree -p codex-router-host --edges normal`

Expected: PASS; tree contains no CLI, proxy, state, SQLx, auth, quota,
selection, secret-store, iocraft, or indicatif crate.

- [x] **Step 5: Commit the host contract foundation**

```bash
git add Cargo.toml Cargo.lock crates/codex-router-host
git commit -m "feat: add shared host control contract"
```

---

### Task 4: Implement retained router and app-server process owners

**Requirements:** R2–R6, R8, R10; F1, F2, F5–F7; V2–V5, V7, V10.

**Files:**

- Create: `crates/codex-router-host/src/{process,router,app_server}.rs`
- Create: `crates/codex-router-host/tests/process_lifecycle.rs`
- Modify: `crates/codex-router-host/src/lib.rs`
- Create: `crates/codex-router-test-support/src/shared_host.rs`
- Modify: `crates/codex-router-test-support/src/lib.rs`

**Interfaces:**

```rust
pub enum RouterOwnership { External, Owned }
pub struct RouterChild { child: tokio::process::Child, process_group_id: u32 }
pub struct AppServerChild { child: tokio::process::Child, identity: ExecutableIdentity, shutdown: ShutdownProgress }
pub struct ExpectedExit { pub child_id: u32, pub term_sent: bool, pub kill_sent: bool }
pub enum ShutdownOutcome { Graceful, Forced, TimedOutStillRunning }
```

- [x] **Step 1: Write failing fixture-driven lifecycle tests**

Use permanent fixture modes in test-support: ready HTTP router, incompatible
listener, auth-required router, signal-recording child, delayed exit, never
exit, native app-server websocket fixture, and socket squatter. Assert separate
process groups, exact PID signals, no external process signal, no replacement
before old exit, one SIGTERM, pinned SIGKILL at 60 seconds, stop at 70 seconds,
retained handle after timeout, and later reap without another signal.

- [x] **Step 2: Run and observe lifecycle failures**

Run: `cargo nextest run -p codex-router-host --test process_lifecycle`

Expected: FAIL on missing owners.

- [x] **Step 3: Implement process owners with injected clock/probe seams**

Use `tokio::process::Command`, `std::os::unix::process::CommandExt::process_group(0)`,
`rustix::process` signal helpers, and async child waits. Router start runs the
current executable's existing `serve` command and accepts only the exact health
schema with auth disabled. App-server readiness delegates to
`codex-router-codex`; foreign endpoints fail closed.

The shared app-server shutdown routine stores `ExpectedExit` immediately before
the first signal. A timed-out child and its progress remain in owner state. The
router stop boundary is SIGTERM plus ten seconds with no SIGKILL.

- [x] **Step 4: Run focused real-Unix tests**

Run: `cargo nextest run -p codex-router-host --test process_lifecycle`

Expected: PASS with fake-clock policy tests and bounded real-signal tests.

- [x] **Step 5: Commit process ownership**

```bash
git add crates/codex-router-host crates/codex-router-test-support
git commit -m "feat: own shared host child lifecycles"
```

---

### Task 5: Implement the single-owner Tokio runtime and bounded recovery

**Requirements:** R6, R8–R10; C1; F1, F2, F6, F7; V5, V7, V8, V10.

**Files:**

- Create: `crates/codex-router-host/src/{runtime,telemetry}.rs`
- Modify: `crates/codex-router-host/src/lib.rs`
- Modify: `crates/codex-router-host/tests/operator_runtime.rs`

**Interfaces:**

```rust
pub struct HostRuntime;

impl HostRuntime {
    pub async fn run(config: HostConfig, dependencies: HostDependencies) -> Result<HostExit, HostError>;
}

pub async fn send_operator_request(
    socket: &Path,
    request: OperatorRequest,
    deadline: Duration,
) -> Result<Vec<OperatorFrame>, OperatorClientError>;
```

- [ ] **Step 1: Write failing state-transition and concurrency tests**

Assert 30-second startup composition, immediate busy for overlapping mutations,
status during mutation, caller disconnect not cancelling crossed side effects,
one steady-state recovery attempt, second-exit exhaustion, no nested recovery
during explicit operations, explicit native-ready restart resetting the budget
for both connected and degraded Remote Control, router exit isolation, and zero
probe calls during an idle interval.

- [ ] **Step 2: Run and observe the missing-runtime failures**

Run: `cargo nextest run -p codex-router-host runtime recovery idle busy`

Expected: FAIL.

- [ ] **Step 3: Implement one owner task**

Use one bounded `mpsc` channel (capacity 8) for admitted operator work and
`oneshot` replies. Per-connection tasks decode one bounded request only.
`tokio::select!` converges requests, retained child exits, Unix SIGINT/SIGTERM/
SIGHUP, retained identity work, and deadlines. Store progress before awaiting
each cancellable boundary. No `Arc<Mutex<HostState>>` is permitted.

Startup order is compatible router → app-server native readiness → Remote
Control observation → terminal snapshot. Foreground stop latches no-replacement
intent and performs the exact cleanup order from Program Design.

- [ ] **Step 4: Add safe lifecycle telemetry**

Record operation/result/duration, ownership class, readiness class, recovery
budget, and installed/running relation. Never record paths, hashes, argv,
environment, updater output, prompts, credentials, protocol frames, or raw
errors without redaction.

- [ ] **Step 5: Run runtime tests under lint**

Run: `cargo clippy -p codex-router-host --all-targets -- -D warnings`

Run: `cargo nextest run -p codex-router-host runtime recovery idle busy`

Expected: PASS.

- [ ] **Step 6: Commit the runtime/recovery slice**

```bash
git add crates/codex-router-host
git commit -m "feat: run bounded shared host recovery"
```

---

### Task 6: Implement explicit app-server and owned-router restart operations

**Requirements:** R6, R8, R9; F1, F2, F5, F6; V5, V7, V8.

**Files:**

- Modify: `crates/codex-router-host/src/{runtime,router,app_server,domain}.rs`
- Modify: `crates/codex-router-host/tests/{operator_runtime,process_lifecycle}.rs`

- [ ] **Step 1: Add failing restart matrix tests**

Cover graceful/forced/timed-out app-server restart, restart after later child
exit, blocked restart while retained child remains, foreground stop during old
shutdown/replacement start, external-router not-owned, owned-router success,
owned-router timeout, owned-router start failure, unchanged app-server state and
recovery budget during router restart, and model request reaching the restarted
router.

- [ ] **Step 2: Run the restart tests and observe expected failures**

Run: `cargo nextest run -p codex-router-host restart`

- [ ] **Step 3: Implement minimal serialized restart transitions**

App-server restart uses the shared exact-child shutdown path, spawns at most one
replacement, resets recovery only at native readiness, and carries Remote
Control separately. Router restart operates only on its retained owned child
and never touches app-server/recovery dimensions.

- [ ] **Step 4: Run restart tests**

Run: `cargo nextest run -p codex-router-host restart`

Expected: PASS.

- [ ] **Step 5: Commit explicit restart behavior**

```bash
git add crates/codex-router-host
git commit -m "feat: add explicit shared host restarts"
```

---

### Task 7: Implement the four-result updater and foreground re-exec

**Requirements:** R7–R10; C1, C2; F3–F5; V6, V8, V10.

**Files:**

- Create: `crates/codex-router-host/src/update.rs`
- Create: `crates/codex-router-host/tests/update_reexec.rs`
- Modify: `crates/codex-router-host/src/{lib,runtime,domain,instance}.rs`

**Interfaces:**

```rust
pub enum UpdateResult {
    NoChange,
    FailedWithoutRestart { message: String },
    UpdatedAndHostRestarted { snapshot: HostSnapshot },
    UpdatedButReplacementFailed { message: String, recovery_action: String },
}
```

- [ ] **Step 1: Write the failing update matrix**

Cover initial identity error/timeout, updater exit failure, updater timeout and
retained reap, second updater rejected until reap, post-updater identity error/
timeout, unchanged identity, changed identity, app-server teardown failure,
router teardown failure, telemetry flush success/failure/timeout, exec failure,
bootstrap validation failure, socket-before-readiness, replacement success,
operator publication timeout, 40-second convergence timeout, differing `PATH`,
and manual launch within the reconnect window.

Every pre-change failure/no-change assertion proves app-server PID and an open
client connection remain unchanged. Every update invocation assertion proves
the exact captured managed executable receives `update`.

- [ ] **Step 2: Run and observe missing-update failures**

Run: `cargo nextest run -p codex-router-host --test update_reexec`

Expected: FAIL.

- [ ] **Step 3: Implement updater containment and identity ordering**

Resolve/hash before update, invoke that same path, resolve/hash the same path
after success, and signal no children until a changed identity is proven. The
updater has a 15-minute deadline, then SIGTERM to its exact process group, a
ten-second wait, SIGKILL, retained handle, and no activation.

- [ ] **Step 4: Implement changed-update terminal sequence**

Send `replacement-starting`; reject new mutations; stop app-server then owned
router; close operator tasks; remove operator socket; bounded best-effort
telemetry shutdown; duplicate the held lock onto stdin; clear stdin CLOEXEC;
exec the current `codex-router host` argv with private inherited-lock bootstrap.
Replacement validates the inherited descriptor, restores CLOEXEC before any
child spawn, publishes the operator socket, and converges startup.

If exec returns, release authority and exit nonzero. Never resume the old
runtime or invent a fifth result.

- [ ] **Step 5: Run update and FD-inheritance tests**

Run: `cargo nextest run -p codex-router-host --test update_reexec`

Expected: PASS, including proof that ordinary router/app-server/updater children
cannot retain singleton authority.

- [ ] **Step 6: Commit update/re-exec behavior**

```bash
git add crates/codex-router-host
git commit -m "feat: conditionally update and reexec host"
```

---

### Task 8: Compose `codex-router host`, sessions attachment, and presentation

**Requirements:** R1–R3, R6, R7, R9; C1–C3; V1, V2, V6, V8, V9.

**Files:**

- Create: `crates/codex-router-cli/src/host.rs`
- Create: `crates/codex-router-cli/src/presentation/host.rs`
- Modify: `crates/codex-router-cli/src/{lib,sessions,telemetry}.rs`
- Modify: `crates/codex-router-cli/src/presentation/mod.rs`
- Modify: `crates/codex-router-cli/Cargo.toml`

**CLI contract:**

```text
codex-router host [--router-root PATH] [--port PORT]
codex-router host status [--router-root PATH]
codex-router host restart [--router-root PATH]
codex-router host restart-router [--router-root PATH]
codex-router host update [--router-root PATH]
```

- [ ] **Step 1: Write failing parser/dispatch/render/session tests**

Assert `host` joins native async dispatch, resolved coordination artifacts use
router root while native socket uses normal Codex home, host output is
deterministic under non-TTY, typed renderer contains all mandatory status
dimensions and no canaries, changed update reconnects and sends
`await-host-start`, and sessions new/resume argv contain root-level
`--remote unix://` without calling the operator socket.

- [ ] **Step 2: Run and observe missing CLI failures**

Run: `cargo nextest run -p codex-router-cli host sessions_remote`

Expected: FAIL.

- [ ] **Step 3: Implement native async CLI dispatch and path composition**

Add `CliCommand::Host(HostCommand)` to the async dispatch set. `host` without a
subcommand acquires authority and runs the foreground runtime; other subcommands
are bounded operator clients. Do not run host through `run_with_io`'s sync
worker or create another Tokio runtime.

Resolve coordination paths beneath `router_root_or_default`; resolve Codex home
from the normal Codex environment independently. Sessions use the adapter's
same conventional remote projection for both new and resume.

- [ ] **Step 4: Implement presentation without lifecycle ownership**

Create typed `HostStatusViewModel` and pure line renderers. Use indicatif only
for the changed-update bounded wait when both streams are interactive; otherwise
emit deterministic progress/result lines. Do not add fullscreen UI unless an
existing interaction truly requires it. If iocraft is used, layout must use
nested `View`/`Text`, flex/gap/margin/padding, and separate content/spacer/
shortcut siblings.

- [ ] **Step 5: Expose bounded explicit telemetry shutdown**

`TelemetryGuard` returns a clone-only `TelemetryShutdownHandle` consumed by the
host pre-exec adapter. It force-flushes/shuts down once in `spawn_blocking` with
no lifecycle authority captured. Ordinary Drop behavior remains for all normal
returns.

- [ ] **Step 6: Run CLI and presentation tests**

Run: `cargo nextest run -p codex-router-cli host sessions_remote presentation`

Expected: PASS.

- [ ] **Step 7: Commit CLI composition**

```bash
git add crates/codex-router-cli
git commit -m "feat: expose shared Codex host commands"
```

---

### Task 9: Add end-to-end host proof fixtures and acceptance commands

**Requirements:** V1–V10.

**Files:**

- Modify: `crates/codex-router-test-support/src/shared_host.rs`
- Create: `crates/codex-router-test-support/tests/shared_host.rs`
- Modify: `crates/codex-router-test-support/Cargo.toml`
- Modify: `README.md` only if it already owns CLI command documentation;
  otherwise add no permanent documentation outside the accepted spec folder.

- [ ] **Step 1: Add a failing real binary smoke test**

The test launches a debug `codex-router host` with isolated router root, a
fixture Codex home/executable, and fixture upstream. It waits on protocol
readiness (never sleeps blindly), runs status, exercises one app-server restart,
one router restart, one crash recovery plus exhaustion, each update outcome,
foreground SIGINT, and confirms socket/lock cleanup and direct client socket
identity.

- [ ] **Step 2: Run and observe any missing wiring failures**

Run: `cargo nextest run -p codex-router-test-support --test shared_host`

- [ ] **Step 3: Complete only missing in-scope wiring**

Fix implementation defects in existing task ownership. Do not add adoption,
persistence, background polling, alternate protocols, or a second lifecycle
path to satisfy the smoke test.

- [ ] **Step 4: Run the complete automated proof matrix**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
```

Expected: all exit 0 with pass counts recorded.

- [ ] **Step 5: Run safe manual/runtime proof**

Use only isolated debug router state and a non-production port. Do not replace
the production router process. Record:

```text
codex-router serve / sessions / host help transcript
GET /healthz compatible, incompatible, auth-required, absent cases
direct CLI attachment to the same app-server socket
host status and explicit app-server/router restart
one unexpected recovery and visible exhaustion
no-change and failed updater preserving an open client
changed-update re-exec/reconnect with the fixture managed install
Remote Control status and, when credentials/network permit, one real operation
before and after ordinary restart
exact installed Desktop attachment result or explicit external blocker
bounded idle CPU/probe-count observation
Victoria trace/metric observation when local OTel stack is available
secret/private-content canary absence
```

- [ ] **Step 6: Commit proof harness and any scoped documentation**

```bash
git add crates/codex-router-test-support README.md
git commit -m "test: prove shared Codex host lifecycle"
```

Omit `README.md` from staging when unchanged.

---

### Task 10: Independent implementation review, one remediation, and PR readiness

**Requirements:** User delivery gate; no product requirement is added here.

**Files:** Current branch diff only; review receipts go under ignored `tmp/`,
not product documentation.

- [ ] **Step 1: Verify the review input is current**

Record exact HEAD, `git status --short`, `git diff --check`, changed files,
Requirements/Specification/Program Design hashes, and the four automated proof
commands plus manual proof transcript.

- [ ] **Step 2: Dispatch two independent implementation reviews**

Use `shravan-dev-workflow:manage-agents` and
`shravan-dev-workflow:implementation-review-swarm`:

```text
Reviewer A: Claude Opus 5, no shared conversation history
Reviewer B: GPT-5.6-Sol, reasoning=high, no shared conversation history
```

Each receives the authoritative artifacts, complete current diff, exact proof,
scope/non-goals, and asks for requirement → source → failure evidence. Do not
silently substitute either requested model.

- [ ] **Step 3: Synthesize and validate findings**

Accept only reproducible, in-scope findings grounded in current source and a
named obligation. Reject speculative platform growth, upstream changes,
additional continuity promises, persistence, polling, adoption, or proxying.

- [ ] **Step 4: Perform exactly one bounded remediation cycle**

Write a failing regression test for each accepted behavioral defect, implement
the smallest fix in the existing owner, and rerun affected focused tests. Do not
start a second remediation cycle; unresolved valid findings remain explicit PR
blockers.

- [ ] **Step 5: Rerun final proof after remediation**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
git diff --check
```

Repeat affected manual proof and record results separately from external
Desktop/Remote Control gates that are unavailable.

- [ ] **Step 6: Prepare and inspect the unmerged PR**

Use `shravan-dev-workflow:implementation-pr-wrapup`. Intentionally stage only
the accepted implementation/spec/plan files, commit, push, open or update the
PR, then inspect checks, comments, unresolved threads, mergeability, and the
published diff. Stop with the PR ready and unmerged. Do not release or merge.

---

## Requirement-to-task proof index

| Requirement | Implementation tasks | Primary proof |
| --- | --- | --- |
| R1 | 3, 5, 8 | CLI boundary transcript and async dispatch |
| R2 | 2, 4, 8, 9 | direct socket/process correlation |
| R3 | 2, 8, 9 | real CLI/Desktop/Remote Control acceptance |
| R4 | 1, 2, 4, 9 | `/healthz` matrix and hosted model request |
| R5 | 2, 4, 9 | connected/degraded fixtures and real remote operation |
| R6 | 3–6, 8, 9 | singleton, serialization, lifecycle integration |
| R7 | 2, 3, 7–9 | four-result update/re-exec matrix |
| R8 | 4–6, 9 | exact one-attempt recovery and reset |
| R9 | 3, 5, 7–9 | mandatory status, safe rendering, OTel/canaries |
| R10 | all | dependency/call-path inspection and idle observation |

## Plan self-review receipt

- U1–U10, R1–R10, C1–C3, F1–F7, and V1–V10 each map to at least one task and
  proof seam.
- Type/signature ownership is one-way: CLI → host → Codex/core; Codex never
  depends on host/CLI, and host never depends on CLI/proxy/state.
- No placeholder (`TBD`, `TODO`, “implement later”, compatibility shim, or
  unspecified error handling) remains.
- Deletion test holds: each new crate/module is tied to the accepted independent
  change axis or named runtime obligation; no database, daemon manager,
  adoption layer, polling service, client proxy, or generation coordinator was
  introduced.
- The plan stops at PR-ready/unmerged and schedules exactly one post-review
  remediation cycle.
