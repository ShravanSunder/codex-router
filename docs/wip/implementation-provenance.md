# Implementation Provenance

Date: 2026-06-20
Workflow: `shravan-dev-workflow:implementation-execute-plan`
Plan: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/plans/2026-06-20-codex-router-implementation-plan.md`

## Plan Coverage

- Plan line count: 861.
- Plan chunks read: 1-220, 221-440, 441-660, 661-861.
- Spec line count: 450.
- Research evidence line count: 94.
- Spec review line count: 66.
- README line count: 15.

## Codex Source

Path:
`/Users/shravansunder/Documents/dev/open-source/ai-harness/codex`

Read-only commands:

```shell
git status --short --branch
git rev-parse HEAD
git remote -v
git -c log.showSignature=false log -1 --format='%H%n%cd%n%s' --date=iso-strict
```

Observed:

```text
## main...origin/main
d66708232299bdbf373ec55b0d6b938c246cfa60
origin https://github.com/openai/codex.git (fetch)
origin https://github.com/openai/codex.git (push)
d66708232299bdbf373ec55b0d6b938c246cfa60
2026-06-19T21:37:01-07:00
Allow resume and settings commands during tasks and MCP startup (#29154)
```

No `git fetch`, `git pull`, or external checkout mutation was run.

## Prodex Source

Path:
`/Users/shravansunder/Documents/dev/open-source/ai-dev/prodex`

Read-only commands:

```shell
git status --short --branch
git rev-parse HEAD
git remote -v
git -c log.showSignature=false log -1 --format='%H%n%cd%n%s' --date=iso-strict
```

Observed:

```text
## main...origin/main
682e442a11b0c3e7c2d0264694d77ff259c15312
origin https://github.com/christiandoxa/prodex.git (fetch)
origin https://github.com/christiandoxa/prodex.git (push)
682e442a11b0c3e7c2d0264694d77ff259c15312
2026-06-20T12:28:12+07:00
chore(release): release 0.198.0
```

No `git fetch`, `git pull`, or external checkout mutation was run.

## Codex Manual

Command:

```shell
node /Users/shravansunder/.codex/skills/.system/openai-docs/scripts/fetch-codex-manual.mjs
```

Observed:

```text
Manual path: /var/folders/4f/697ggy6x26q8kh9qb2js4xnc0000gn/T/openai-docs-cache/codex-manual.md
Outline path: /var/folders/4f/697ggy6x26q8kh9qb2js4xnc0000gn/T/openai-docs-cache/codex-manual.outline.md
Manual status: local manual was already current.
```

## Installed Codex

Commands:

```shell
codex --version
codex debug models --help
codex --profile codex-router exec --help
```

Observed:

```text
codex-cli 0.141.0
```

`codex debug models --help` describes a raw model-catalog command and accepts
`--bundled`, `-c/--config`, feature flags, and help. It is not the installed
profile smoke path.

`codex --profile codex-router exec --help` accepts the runtime profile flag and
is the command family the smoke must use.

## Host Toolchain

Read-only checks:

```shell
command -v rustup rustc cargo cargo-nextest cargo-deny cargo-audit actionlint
rustup toolchain list
rustup which cargo --toolchain 1.95.0
rustup run 1.95.0 rustc --version
rustup run 1.95.0 cargo --version
rustup run 1.95.0 cargo nextest --version
rustup run 1.95.0 cargo deny --version
rustup run 1.95.0 cargo audit --version
/opt/homebrew/bin/actionlint --version
```

Observed:

```text
/opt/homebrew/bin/rustup
stable-aarch64-apple-darwin (active, default)
1.81.0-aarch64-apple-darwin
1.95.0-aarch64-apple-darwin
/Users/shravansunder/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
```

Missing required tools:

```text
rustup run 1.95.0 cargo nextest --version
error: command failed: 'cargo nextest --version': No such file or directory (os error 2)
exit=1

rustup run 1.95.0 cargo deny --version
error: command failed: 'cargo deny --version': No such file or directory (os error 2)
exit=1

rustup run 1.95.0 cargo audit --version
error: command failed: 'cargo audit --version': No such file or directory (os error 2)
exit=1

/opt/homebrew/bin/actionlint --version
zsh:1: no such file or directory: /opt/homebrew/bin/actionlint
exit=127
```

## T0 Decision

T0 source provenance is complete enough to identify the next gate. T1 must not
start until host-bootstrap approval is explicit, because installing
`cargo-nextest`, `cargo-deny`, `cargo-audit`, or `actionlint` mutates
`~/.cargo` or the Homebrew prefix.

The pinned Rust compiler and cargo are available through:

```shell
rustup run 1.95.0 cargo
rustup run 1.95.0 rustc
```

The missing approval-sensitive commands are:

```shell
rustup run 1.95.0 cargo install cargo-nextest --locked
rustup run 1.95.0 cargo install cargo-deny --locked
rustup run 1.95.0 cargo install cargo-audit --locked
brew install actionlint
```

## Host Tool Recheck

Timestamp: 2026-06-20T15:30:45Z

Commands:

```shell
command -v rustup rustc cargo cargo-nextest cargo-deny cargo-audit actionlint
rustup run 1.95.0 rustc --version
rustup run 1.95.0 cargo --version
rustup run 1.95.0 cargo nextest --version
rustup run 1.95.0 cargo deny --version
rustup run 1.95.0 cargo audit --version
actionlint --version
```

Observed:

```text
/opt/homebrew/bin/rustup
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
cargo nextest: no such command, exit 101
cargo deny: no such command, exit 101
cargo audit: no such command, exit 101
actionlint: command not found, exit 127
```

Decision remains unchanged: T1 must not begin until the missing proof tools are
installed with explicit approval, or the reviewed plan is revised.

## Host Bootstrap Completion

Timestamp: 2026-06-20T18:00:21Z

Approved by user:

```text
feel fre to install pcakges kthat are standrd going forwrad no neef or approval.

for brew thansk for askign you showuld always ask me.  and i giv eyoku  approval
```

Commands run:

```shell
RUSTC="$(rustup which rustc --toolchain 1.95.0)" "$(rustup which cargo --toolchain 1.95.0)" install cargo-nextest --locked
RUSTC="$(rustup which rustc --toolchain 1.95.0)" "$(rustup which cargo --toolchain 1.95.0)" install cargo-deny --locked
RUSTC="$(rustup which rustc --toolchain 1.95.0)" "$(rustup which cargo --toolchain 1.95.0)" install cargo-audit --locked
brew install actionlint
```

Why direct pinned Cargo was used:

`rustup run 1.95.0 cargo install ...` failed because the local
`~/.cargo/bin/cargo` and `~/.cargo/bin/rustc` proxy symlinks point at a removed
`/opt/homebrew/bin/rustup-init`. The pinned toolchain binaries are valid, so the
install used direct `rustup which cargo --toolchain 1.95.0` and
`RUSTC="$(rustup which rustc --toolchain 1.95.0)"`.

Installed versions:

```text
cargo-nextest 0.9.137
cargo-deny 0.19.9
cargo-audit 0.22.2
actionlint 1.7.12
shellcheck 0.11.0
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
```

Homebrew also installed `shellcheck` as an `actionlint` dependency.

T1 may proceed using the resolved pinned Cargo binary:

```shell
RUSTC="$(rustup which rustc --toolchain 1.95.0)" "$(rustup which cargo --toolchain 1.95.0)" <args>
```

## T1 Rustfmt Adjustment

Stable `rustfmt` from toolchain `1.95.0` warned that
`imports_granularity = "Item"` is unstable. The implementation plan and
`rustfmt.toml` were updated to use only stable-supported rustfmt settings so
the T1 quality gate can stay warning-clean.

## T1 Workspace Baseline Proof

Timestamp: 2026-06-20T18:04:17Z

Created:

- `Cargo.toml`
- `rust-toolchain.toml`
- `rustfmt.toml`
- `deny.toml`
- `.cargo/audit.toml`
- `.github/workflows/ci.yml`
- `.gitignore`
- minimal crates: `codex-router-auth`, `codex-router-cli`,
  `codex-router-core`, `codex-router-proxy`, `codex-router-quota`,
  `codex-router-secret-store`, `codex-router-selection`,
  `codex-router-state`, and `codex-router-test-support`
- `tests/smoke/.gitkeep`

Resolved tool invocation:

```shell
TOOLCHAIN_BIN="$(dirname "$(rustup which cargo --toolchain 1.95.0)")"
PATH="$TOOLCHAIN_BIN:$HOME/.cargo/bin:$PATH"
RUSTC="$TOOLCHAIN_BIN/rustc"
CARGO="$TOOLCHAIN_BIN/cargo"
```

Version proof:

```text
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
cargo-nextest 0.9.137
cargo-deny 0.19.9
cargo-audit 0.22.2
actionlint 1.7.12
```

Quality proof:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 9 tests run, 9 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
```

Guard proof:

```text
forbidden Prodex/Codex-home/session-repair pattern search: pass
quota normal/build dependency on codex-router-secret-store: absent
non-test dependency on codex-router-test-support: absent
```

Note:

The first full sweep found `tests/` missing, which made the guard command noisy.
`tests/smoke/.gitkeep` was added and the guard was rerun successfully.

## T2 Red/Green Notes

T2 followed a red/green loop:

- Red: `cargo nextest run -p codex-router-core` failed because the new tests
  referenced missing `audit`, `config`, `error`, `ids`, and `redaction`
  modules.
- Green: the minimal core modules were implemented and
  `cargo nextest run -p codex-router-core` passed with 6 tests.

The first T2 full sweep failed at `cargo deny check` because `unicode-ident`
introduced the OSI-approved `Unicode-3.0` license through `serde_derive` /
`thiserror-impl`. `Unicode-3.0` was added to `deny.toml` only after it was
encountered by the actual dependency graph.

## T2 Core Primitive Proof

Timestamp: 2026-06-20T18:09:08Z

Implemented:

- `config.rs`: deny-unknown TOML config, loopback-only listener validation,
  local-token env field, router-root field, and private file audit sink field.
- `ids.rs`: newtypes for account, request, reservation, affinity, token
  generation, and route ids.
- `redaction.rs`: `SecretString` with redacted `Debug`, `Display`, and
  serialization plus explicit `expose_secret`.
- `audit.rs`: redacted allowlist audit event schema with route kind and outcome.
- `error.rs`: typed config and id errors.

TDD proof:

```text
RED: cargo nextest run -p codex-router-core
     failed because audit/config/error/ids/redaction modules were missing.

GREEN: cargo nextest run -p codex-router-core
       6 tests run, 6 passed, 0 skipped.
```

Full proof after T2:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 14 tests run, 14 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Dependency adjustment:

`toml` was updated from `0.9.12` to `1.1.2` after `cargo deny check` reported
a duplicate `winnow` warning through the older dependency graph. The final T2
sweep passed warning-clean.

## T3 Secret Store And Refresh Lease Proof

Timestamp: 2026-06-20T18:13:42Z

Implemented:

- `codex-router-secret-store/src/model.rs`
  - `SecretKey`
  - `SecretStoreError`
- `codex-router-secret-store/src/file_backend.rs`
  - `SecretStore` trait
  - `FileSecretStore`
  - private root creation
  - `.codex` path rejection
  - symlink rejection for root, parent path, temp path, and target secret file
  - temp-write plus sync plus atomic rename
  - Unix private modes: root `0700`, secret file `0600`
- `codex-router-secret-store/src/refresh_lease.rs`
  - deterministic `ManualClock`
  - owner/follower lease acquisition
  - stale lease recovery
  - owner-matched release

TDD proof:

```text
RED: cargo nextest run -p codex-router-secret-store
     failed because file_backend/model/refresh_lease modules were missing.

GREEN: cargo nextest run -p codex-router-secret-store
       7 tests run, 7 passed, 0 skipped.
```

Full proof after T3:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 20 tests run, 20 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

## T4 Local Router Auth And Token Export Proof

Timestamp: 2026-06-20T19:02:15Z

Implemented:

- `codex-router-core/src/local_auth.rs`
  - `LocalRouterTokenRecord`
  - `LocalRouterAuth`
  - `LocalAuthError`
  - current-token validation before routing
  - missing, empty, wrong, and old-token rejection
  - token redaction in debug output while keeping generation visible
- `codex-router-core/src/ids.rs`
  - `TokenGeneration::new`
  - `TokenGeneration::next`
  - `TokenGeneration::as_u64`
- `codex-router-proxy/src/local_auth.rs`
  - `ProxyLocalAuthGate`
  - proxy-local authorization gate before selection/upstream work
- `codex-router-cli/src/token.rs`
  - `LocalRouterTokenService`
  - real `SecretStore`-backed token rotation
  - token generation metadata persisted in the secret store
  - POSIX shell-safe `CODEX_ROUTER_TOKEN='...'` assignment generation
  - export helper that emits only one assignment and no prose

TDD proof:

```text
RED: cargo nextest run -p codex-router-core -p codex-router-proxy -p codex-router-cli
     failed because local_auth modules and TokenGeneration constructors were missing.

GREEN: cargo nextest run -p codex-router-core -p codex-router-proxy -p codex-router-cli
       13 tests run, 13 passed, 0 skipped.
```

Full proof after T4:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 25 tests run, 25 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- T4 implemented the token state and proxy auth gate primitive. Forced close of
  old-token WebSockets remains a later proxy transport behavior to prove when
  WebSocket connection ownership exists.

## T5 OAuth Account Store, SQLite Metadata, And Quota Snapshot Model Proof

Timestamp: 2026-06-20T19:29:48Z

Implemented:

- `codex-router-auth/src/oauth.rs`
  - deterministic token expiry and refresh-window classification
  - OpenAI OAuth refresh response classification
  - no generic multi-provider abstraction
- `codex-router-quota/src/snapshot.rs`
  - quota snapshot source model
  - route-band headroom model
  - snapshot freshness classification with stale penalty and unknown state
- `codex-router-state/src/account.rs`
  - non-secret account metadata DTO
  - enabled/disabled account status parsing
- `codex-router-state/src/quota_snapshot.rs`
  - SQLite quota snapshot DTO with source, observed time, route band,
    remaining headroom, reset hint, and stale penalty
- `codex-router-state/src/sqlite.rs`
  - SQLite open and v1 migration
  - schema version enforcement
  - account upsert/load
  - quota snapshot upsert/load
  - corrupt account row isolation with redacted diagnostics

Dependency decision:

- Added `rusqlite 0.40.1` to the state crate with `default-features = false`
  and `features = ["bundled"]`.
- `rusqlite` defaults were explicitly disabled after `cargo deny check`
  revealed the default wasm/VFS side branch brought in a Zlib dependency and
  duplicate `hashbrown`. The final graph keeps bundled SQLite without that
  extra branch.

TDD proof:

```text
RED: cargo nextest run -p codex-router-state -p codex-router-auth -p codex-router-quota
     failed because oauth, snapshot, account, quota_snapshot, and sqlite modules were missing,
     and rusqlite was not yet linked.

GREEN: cargo nextest run -p codex-router-state -p codex-router-auth -p codex-router-quota
       10 tests run, 10 passed, 0 skipped.
```

Full proof after T5:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 32 tests run, 32 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- T5 proves the metadata and classification primitives. Real OAuth login,
  mock HTTP authorization/token/quota endpoint flows, and background refresh
  workers remain future slices.

## T6 Selection And Routing State Machine Proof

Timestamp: 2026-06-20T19:52:08Z

Implemented:

- `codex-router-selection/src/eligibility.rs`
  - account eligibility classification
  - stale and unknown quota penalties only when known-fresh accounts exist
- `codex-router-selection/src/weighted_deficit.rs`
  - smooth weighted deficit selector over eligible accounts
- `codex-router-selection/src/reservation.rs`
  - transient reservation accounting that reduces immediate headroom
- `codex-router-selection/src/affinity.rs`
  - previous-response affinity pins that resolve only while pinned account is
    eligible
- `codex-router-selection/src/turn_state.rs`
  - HMAC-SHA256 signed turn-state envelopes
  - account pin plus optional upstream token material
  - redacted envelope debug output
  - tamper rejection
- `codex-router-selection/src/precommit.rs`
  - narrow precommit rotation classifier
  - rotation only for auth rejection or quota exhaustion
  - timeout and malformed-response failures return to Codex instead of becoming
    router policy gates

Dependency decision:

- Added `base64`, `hmac`, `sha2`, `serde`, and `serde_json` for signed
  turn-state envelopes.
- Added `BSD-3-Clause` to `deny.toml` because `subtle`, pulled through the
  standard RustCrypto HMAC/SHA stack, uses that OSI-approved license.

TDD proof:

```text
RED: cargo nextest run -p codex-router-selection
     failed because affinity, eligibility, precommit, reservation, turn_state,
     weighted_deficit modules, quota dependency, and id helpers were missing.

GREEN: cargo nextest run -p codex-router-selection
       7 tests run, 7 passed, 0 skipped.
```

Full proof after T6:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 38 tests run, 38 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- T6 proves the in-process selection state machine. It does not yet wire
  selection into the proxy transport or persistent reservation/affinity
  repositories.

## T7 Background Refresh Runtime Proof

Timestamp: 2026-06-20T20:17:31Z

Implemented:

- `codex-router-quota/src/worker.rs`
  - `QuotaSnapshotReader`
  - `RefreshScheduler`
  - `QuotaRefreshRuntime`
  - startup path that returns existing snapshots immediately and schedules
    account refresh without inline provider work
- `codex-router-auth/src/refresh_worker.rs`
  - non-secret account refresh inputs
  - refresh/skip decisions from deterministic token expiry classification
- `codex-router-cli/src/doctor.rs`
  - doctor report DTOs
  - stale/missing/fresh quota rendering
  - secret canary redaction in rendered output

TDD proof:

```text
RED: cargo nextest run -p codex-router-quota -p codex-router-auth -p codex-router-cli
     failed because worker, refresh_worker, and doctor modules were missing.

GREEN: cargo nextest run -p codex-router-quota -p codex-router-auth -p codex-router-cli
       12 tests run, 12 passed, 0 skipped.
```

Full proof after T7:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 41 tests run, 41 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- T7 proves deterministic refresh scheduling and redacted diagnostics. It does
  not yet run a real async/threaded background worker, call live OAuth/quota
  endpoints, or wire refresh into the proxy server lifecycle.

## T7.5 Contract Freeze Before Proxy Integration Proof

Timestamp: 2026-06-20T20:39:12Z

Implemented:

- `codex-router-auth/src/quota_client.rs`
  - `AuthenticatedQuotaClient`
  - `QuotaFetchRequest`
  - `QuotaFetchResponse`
  - `AuthenticatedQuotaError`
- `codex-router-state/src/repositories.rs`
  - `AccountStateRepository`
  - `QuotaSnapshotRepository`
  - `AffinityRepository`
- `codex-router-state/src/sqlite.rs`
  - SQLite implementations of the state repository contracts
  - `affinity_pins` migration table
- `codex-router-selection/src/weighted_deficit.rs`
  - `SelectionDecision`
- `codex-router-selection/src/reservation.rs`
  - `ReservationHandle`
- `codex-router-selection/src/precommit.rs`
  - `PrecommitFailureClassifier`

TDD proof:

```text
RED: cargo nextest run -p codex-router-auth -p codex-router-state -p codex-router-selection
     failed because quota_client, repositories, SelectionDecision,
     ReservationHandle, and PrecommitFailureClassifier were missing.

GREEN: cargo nextest run -p codex-router-auth -p codex-router-state -p codex-router-selection
       19 tests run, 19 passed, 0 skipped.
```

Full proof after T7.5:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 45 tests run, 45 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- T7.5 freezes proxy-facing contract DTOs and repository traits. It does not
  implement HTTP/SSE/WebSocket proxy transport.

## T8 Partial HTTP/SSE And WebSocket Proxy Protocol Proof

Timestamp: 2026-06-20T20:57:02Z

Implemented:

- `codex-router-proxy/src/routes.rs`
  - required Codex route classification
  - WebSocket upgrade classification for `/v1/responses`
  - fail-closed rejection for unsupported paths, including Realtime/WebRTC
- `codex-router-proxy/src/headers.rs`
  - strips local router token header
  - strips hop-by-hop headers
  - strips client-supplied upstream `Authorization` and cookie auth
  - injects selected upstream auth exactly once
- `codex-router-proxy/src/upstream.rs`
  - upstream request builder
  - body byte preservation without interpreting unknown Codex fields
- `codex-router-test-support/src/transcript.rs`
  - request transcript DTO
- `codex-router-test-support/src/mock_upstream.rs`
  - mock upstream transcript recorder

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy -p codex-router-test-support
     failed because headers, routes, upstream, mock_upstream, and transcript
     modules were missing.

GREEN: cargo nextest run -p codex-router-proxy -p codex-router-test-support
       6 tests run, 6 passed, 0 skipped.
```

Full proof after T8 partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 48 tests run, 48 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This is a T8 protocol-transform slice only. It does not yet implement a
  loopback HTTP server, SSE streaming, WebSocket first-frame routing, upstream
  handshakes, pre-selection close behavior, or end-to-end transcript tests.

## T8 Partial HTTP/SSE Handler Proof

Timestamp: 2026-06-20T21:18:44Z

Implemented:

- `codex-router-proxy/src/http_sse.rs`
  - `HttpProxyRequest`
  - `UpstreamHttpRequest`
  - `HttpProxyResponse`
  - `UpstreamHttpTransport`
  - `HttpProxyService`
  - fail-closed unsupported-route handling before upstream work
  - supported route forwarding through an injected upstream transport
  - request body preservation for Responses/SSE-style payloads
  - upstream response status, headers, and body preservation
- `codex-router-proxy/src/lib.rs`
  - protocol tests for models `ETag` preservation
  - protocol tests for Responses body preservation and allowed header forwarding
  - protocol tests proving unsupported paths do not call upstream

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because http_sse module was missing.

GREEN: cargo nextest run -p codex-router-proxy
       7 tests run, 7 passed, 0 skipped.
```

Full proof after T8 HTTP/SSE handler partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 51 tests run, 51 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice still does not bind a loopback network server or implement
  WebSocket first-frame routing, WebSocket handshake header tests, or hostile
  pre-selection close behavior.

## T8 Partial WebSocket First-Frame Protocol Proof

Timestamp: 2026-06-20T21:34:09Z

Implemented:

- `codex-router-proxy/src/websocket.rs`
  - `WebSocketFrame`
  - `FirstFramePolicy`
  - `WebSocketHandshakeRequest`
  - `WebSocketFirstFrameDecision`
  - `WebSocketCloseReason`
  - `WebSocketProtocolRouter`
  - text-only first frame handling
  - max first-frame byte guard
  - structured JSON parsing for `response.create`
  - malformed/non-`response.create` close classification
  - sanitized upstream handshake headers with selected auth injected exactly
    once
  - unchanged first-frame forwarding after selection

Dependency decision:

- Added `serde_json` to `codex-router-proxy` for structured first-frame JSON
  classification instead of ad hoc string matching.

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because websocket module was missing.

GREEN: cargo nextest run -p codex-router-proxy
       9 tests run, 9 passed, 0 skipped.
```

Full proof after T8 WebSocket first-frame partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 53 tests run, 53 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice proves WebSocket first-frame protocol decisions and handshake
  header sanitization, but still does not bind a network server or open real
  upstream WebSocket connections.

## T8 Partial Loopback Server Bind Runtime Proof

Timestamp: 2026-06-20T18:58:37Z

Implemented:

- `codex-router-proxy/src/server.rs`
  - `LoopbackBindAddress`
  - `LoopbackServerRuntime`
  - `ServerBindError`
  - deterministic `localhost` alias handling
  - loopback-only validation before binding
  - TCP listener bind with kernel-assigned local address reporting
- `codex-router-proxy/src/lib.rs`
  - loopback bind tests for `127.0.0.1`, `localhost`, and `::1`
  - non-loopback rejection tests for `0.0.0.0`, `::`, and LAN address input

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because server module was missing.

RED: cargo nextest run -p codex-router-proxy
     failed because localhost was not accepted as a loopback alias.

GREEN: cargo nextest run -p codex-router-proxy
       12 tests run, 12 passed, 0 skipped.
```

Full proof after T8 loopback server bind partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 56 tests run, 56 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice proves loopback-only bind/runtime validation, but still does not
  implement a full async HTTP server adapter or real upstream WebSocket
  connections.

## T8 Partial Loopback HTTP Adapter And Query Preservation Proof

Timestamp: 2026-06-20T19:04:26Z

Implemented:

- `codex-router-proxy/src/http_sse.rs`
  - classifies routes on the path component only
  - preserves the original path and query string in the upstream request
- `codex-router-proxy/src/server.rs`
  - `LoopbackHttpAdapter`
  - one accepted HTTP/1.x TCP connection handler
  - `httparse`-based request parsing
  - method/path/header/body conversion into `HttpProxyRequest`
  - response serialization from `HttpProxyResponse`
- `codex-router-proxy/src/lib.rs`
  - path/query preservation test for `/v1/responses?stream=true&cursor=abc`
  - real loopback TCP request/response test through `LoopbackServerRuntime`
- `Cargo.toml` and `codex-router-proxy/Cargo.toml`
  - added `httparse = 1.10.1`

Dependency decision:

- Added `httparse` rather than an ad hoc HTTP parser. `cargo info httparse`
  reported version `1.10.1` with license `MIT OR Apache-2.0`, compatible with
  the repo deny policy.

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because `/v1/responses?stream=true&cursor=abc` was rejected as
     unsupported.

GREEN: cargo nextest run -p codex-router-proxy
       13 tests run, 13 passed, 0 skipped.

RED: cargo nextest run -p codex-router-proxy
     failed because `LoopbackHttpAdapter` was missing.

GREEN: cargo nextest run -p codex-router-proxy
       14 tests run, 14 passed, 0 skipped.
```

Full proof after T8 loopback HTTP adapter partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 58 tests run, 58 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice proves one real loopback HTTP/1.x connection path and query/body
  preservation, but still does not implement the long-running server accept
  loop, router-local bearer auth integration at the network boundary, account
  selection wiring, or real upstream WebSocket connections.

## T8 Partial Network-Bound Local Auth And Selection Proof

Timestamp: 2026-06-20T19:10:19Z

Implemented:

- `codex-router-proxy/src/http_sse.rs`
  - `HttpRequestHandler`
  - `SelectedUpstreamAccount`
  - `UpstreamAccountSelector`
  - `AuthenticatedHttpProxyService`
  - `HttpProxyError::LocalAuth`
  - request path and header-value accessors
- `codex-router-proxy/src/server.rs`
  - `LoopbackHttpAdapter` now depends on a parsed-request handler instead of a
    preselected upstream token
  - missing HTTP method is now an explicit parse error
- `codex-router-proxy/src/lib.rs`
  - local-auth-before-selection test proving missing local token returns local
    auth error and selector/upstream call counts stay zero
  - selected-account test proving successful local auth calls selector once,
    forwards selected upstream auth, and strips the local router token
  - real loopback TCP test now goes through local auth and account selection
    composition rather than a preselected token shortcut

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because AuthenticatedHttpProxyService, HttpRequestHandler,
     SelectedUpstreamAccount, and UpstreamAccountSelector were missing.

GREEN: cargo nextest run -p codex-router-proxy
       16 tests run, 16 passed, 0 skipped.
```

Full proof after T8 network-bound local auth and selection partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 60 tests run, 60 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice wires HTTP local auth and selected upstream account material into
  the network path, but still uses a mock selector. The next proxy slice must
  connect selector decisions to real account/quota/state repositories and/or
  add the long-running accept loop. Real upstream WebSocket connections remain
  open.

## T8 Partial Quota-Aware Selector Adapter Proof

Timestamp: 2026-06-20T19:14:41Z

Implemented:

- `codex-router-proxy/src/http_sse.rs`
  - `QuotaAwareAccountState`
  - `QuotaAwareAccountSelector`
  - `QuotaAwareAccountSelectorError`
  - `HttpProxyError::Selection`
  - account id on `SelectedUpstreamAccount`
- `codex-router-proxy/Cargo.toml`
  - added proxy dependencies on `codex-router-quota` and
    `codex-router-selection`
- `codex-router-proxy/src/lib.rs`
  - concrete selector test proving fresh known quota wins against a larger
    stale account after stale penalty
  - concrete selector test proving zero-headroom accounts fail closed before
    upstream

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because QuotaAwareAccountSelector, QuotaAwareAccountState,
     QuotaAwareAccountSelectorError, codex-router-quota dependency, and
     HttpProxyError::Selection were missing.

GREEN: cargo nextest run -p codex-router-proxy
       18 tests run, 18 passed, 0 skipped.
```

Full proof after T8 quota-aware selector adapter partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 62 tests run, 62 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice connects the proxy selector trait to existing selection/quota
  primitives, but still uses in-memory selector input rather than loading from
  SQLite/account repositories. The long-running accept loop and real upstream
  WebSocket connections remain open.

## T8 Partial Bounded Loopback HTTP Accept Loop Proof

Timestamp: 2026-06-20T19:19:39Z

Implemented:

- `codex-router-proxy/src/server.rs`
  - `LoopbackHttpServer`
  - bounded `serve_connections` accept loop around the existing one-connection
    `LoopbackHttpAdapter`
  - accept failure variant on `ServerConnectionError`
- `codex-router-proxy/src/lib.rs`
  - real TCP test proving two sequential loopback requests are accepted,
    parsed, locally authenticated, routed through the selector/upstream
    boundary, and then the bounded loop exits with the handled count

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because LoopbackHttpServer was not defined in crate::server.

GREEN: cargo nextest run -p codex-router-proxy
       19 tests run, 19 passed, 0 skipped.
```

Full proof after T8 bounded loopback HTTP accept loop partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 63 tests run, 63 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice makes the local loopback HTTP server handle multiple bounded
  requests without adding router-owned timeout, retry, health, or provider
  gating behavior. Repository-loaded selector input and real upstream
  WebSocket connections remain open.

## T8 Partial Repository Hydration Foundation Proof

Timestamp: 2026-06-20T19:23:51Z

Implemented:

- `codex-router-state/src/repositories.rs`
  - `AccountStateRepository::list_accounts`
- `codex-router-state/src/sqlite.rs`
  - deterministic `ORDER BY account_id` account metadata listing
  - shared account-row parser for single-account load and list-account load
- `codex-router-state/src/lib.rs`
  - repository contract test proving account metadata lists in selector-stable
    order and includes disabled accounts for explicit eligibility handling
- `codex-router-secret-store/src/account_tokens.rs`
  - `upstream_access_token_key`
- `codex-router-secret-store/src/lib.rs`
  - secret-key contract test proving upstream OpenAI access tokens are
    namespaced by account id without storing token material in SQLite

TDD proof:

```text
RED: cargo nextest run -p codex-router-state
     failed because AccountStateRepository::list_accounts did not exist.

GREEN: cargo nextest run -p codex-router-state
       6 tests run, 6 passed, 0 skipped.

RED: cargo nextest run -p codex-router-secret-store
     failed because crate::account_tokens::upstream_access_token_key did not
     exist.

GREEN: cargo nextest run -p codex-router-secret-store
       8 tests run, 8 passed, 0 skipped.
```

Full proof after T8 repository hydration foundation partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 65 tests run, 65 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice does not yet build the repository-backed proxy selector. It adds
  the two missing source-of-truth primitives that selector hydration needs:
  deterministic account enumeration from state and deterministic token-key
  lookup in the secret store.

## T8 Partial Repository-Backed Selector Hydration Proof

Timestamp: 2026-06-20T19:29:24Z

Implemented:

- `codex-router-proxy/Cargo.toml`
  - added proxy dependencies on `codex-router-state` and
    `codex-router-secret-store`
- `codex-router-proxy/src/http_sse.rs`
  - `RepositoryBackedAccountSelector`
  - request-time hydration from account metadata, quota snapshots, and
    secret-store upstream access tokens
  - shared weighted selector path for in-memory and repository-backed selector
    inputs
  - redacted `StateUnavailable` and `SecretUnavailable` selection errors
  - disabled accounts skipped before token/quota reads
  - missing per-account access-token files treated as unauthenticated account
    ineligibility, not as capacity
- `codex-router-proxy/src/lib.rs`
  - real SQLite plus real file-secret-store test proving enabled accounts are
    hydrated, disabled accounts are ignored, quota headroom selects the expected
    account, and the selected upstream token comes from the secret store

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because codex-router-proxy had no codex-router-state or
     codex-router-secret-store dependencies and no
     RepositoryBackedAccountSelector.

GREEN: cargo nextest run -p codex-router-proxy
       20 tests run, 20 passed, 0 skipped.
```

Full proof after T8 repository-backed selector hydration partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 66 tests run, 66 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- Repository-backed HTTP account selection is now wired for request-time
  selector input. Real upstream WebSocket connections, full CLI runtime
  assembly, and installed-Codex smoke remain open.

## T8 Partial Upstream Endpoint URL Assembly Proof

Timestamp: 2026-06-20T19:31:43Z

Implemented:

- `codex-router-proxy/src/upstream.rs`
  - `UpstreamEndpoint`
  - `UpstreamEndpointError`
  - URL assembly from provider base URL plus Codex request path
  - duplicate `/v1` avoidance for `https://api.openai.com/v1` plus
    `/v1/...` request paths
  - query-string preservation
- `codex-router-proxy/src/lib.rs`
  - endpoint test proving `/v1/responses?stream=true&cursor=abc` and
    `v1/models` join to the intended upstream URLs

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because UpstreamEndpoint was missing from crate::upstream.

GREEN: cargo nextest run -p codex-router-proxy
       21 tests run, 21 passed, 0 skipped.
```

Full proof after T8 upstream endpoint URL assembly partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 67 tests run, 67 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This slice prepares real upstream HTTP/WebSocket transport URL construction
  without adding an HTTP client, retry, timeout, health, or provider-gating
  behavior. Real upstream connections remain open.

## T8 Partial Local HTTP Upstream Transport Proof

Timestamp: 2026-06-20T19:36:26Z

Implemented:

- `codex-router-proxy/src/upstream.rs`
  - `HttpUpstreamTransport`
  - blocking HTTP/1.1 request write over `std::net::TcpStream`
  - upstream response status/header/body parsing with `httparse`
  - HTTP mock-endpoint authority/path assembly through `UpstreamEndpoint`
  - response `Content-Length` handling
- `codex-router-proxy/src/lib.rs`
  - real TCP mock-upstream test proving:
    - proxy-sanitized request reaches a real local upstream socket
    - selected upstream auth is injected
    - local router token and hostile client auth are stripped
    - query string is preserved
    - upstream status, `ETag`, and body are returned

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because HttpUpstreamTransport was missing from crate::upstream.

GREEN: cargo nextest run -p codex-router-proxy
       22 tests run, 22 passed, 0 skipped.
```

Full proof after T8 local HTTP upstream transport partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 68 tests run, 68 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This is local/mock HTTP upstream transport proof. It does not claim HTTPS
  live OpenAI transport, real OAuth/quota proof, or WebSocket upstream
  connection proof.

## T8 Partial Authenticated WebSocket Selection Proof

Timestamp: 2026-06-20T19:38:41Z

Implemented:

- `codex-router-proxy/src/websocket.rs`
  - `AuthenticatedWebSocketRouter`
  - `WebSocketHandshakeRequest::header_value`
  - `WebSocketCloseReason::LocalAuth`
  - `WebSocketCloseReason::Selection`
  - local auth before account selection
  - account selection using WebSocket `/v1/responses` route context
  - delegation to existing first-frame router after selected upstream token is
    available
- `codex-router-proxy/src/lib.rs`
  - test proving a valid local token selects after first frame and injects the
    selected upstream token
  - test proving a missing local token rejects before selection

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy
     failed because AuthenticatedWebSocketRouter and
     WebSocketCloseReason::LocalAuth were missing.

GREEN: cargo nextest run -p codex-router-proxy
       24 tests run, 24 passed, 0 skipped.
```

Full proof after T8 authenticated WebSocket selection partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 70 tests run, 70 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This proves local WebSocket auth and selection composition before upstream
  open data. It does not yet implement a real upstream WebSocket connection or
  tunnel.

## T9 Partial Codex Profile Helper Proof

Timestamp: 2026-06-20T19:41:52Z

Implemented:

- `codex-router-cli/src/profile.rs`
  - `CodexRouterProfile`
  - `CodexRouterProfileWriter`
  - `ProfileDryRun`
  - `ProfileWriteError`
  - exact Codex custom-provider profile rendering
  - dry-run target/content preview with no filesystem mutation
  - explicit approval gate before profile writes
  - approved writes scoped to caller-provided Codex home
- `codex-router-cli/src/lib.rs`
  - profile render contract test for:
    - `[profiles.codex-router]`
    - `model_provider = "codex-router"`
    - `[model_providers.codex-router]`
    - `base_url = "http://127.0.0.1:<port>/v1"`
    - `wire_api = "responses"`
    - `requires_openai_auth = false`
    - `supports_websockets = true`
    - `env_http_headers = { "X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN" }`
  - temp `CODEX_HOME` dry-run/write test proving no write without approval

TDD proof:

```text
RED: cargo nextest run -p codex-router-cli
     failed because profile module/CodexRouterProfile were missing.

GREEN: cargo nextest run -p codex-router-cli
       5 tests run, 5 passed, 0 skipped.

RED: cargo nextest run -p codex-router-cli
     failed because CodexRouterProfileWriter and ProfileWriteError were missing.

GREEN: cargo nextest run -p codex-router-cli
       6 tests run, 6 passed, 0 skipped.
```

Full proof after T9 profile helper partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 72 tests run, 72 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This adds the render/write helper used by later installed-Codex smoke. It
  does not yet implement full CLI argument parsing or run the installed-Codex
  smoke harness.

## T9 Partial Codex Profile Command Wiring Proof

Timestamp: 2026-06-20T20:12:18Z

Implemented:

- `codex-router-cli/src/lib.rs`
  - process-independent `run_with_io` command entry for tests and future smoke
    harnesses
  - `CliContext` env capture that treats empty `CODEX_ROUTER_TOKEN` as missing
  - `codex-router profile print`
  - `codex-router profile doctor`
  - `codex-router profile write --dry-run`
  - `codex-router profile write --approve-codex-home-write`
  - temp `--codex-home` targeting for profile writes
  - no real `~/.codex` writes in tests

TDD proof:

```text
RED: cargo nextest run -p codex-router-cli profile_
     failed because CliContext and run_with_io were missing.

GREEN: cargo nextest run -p codex-router-cli profile_
       7 profile tests run, 7 passed, 4 skipped by filter.
```

Full proof after T9 profile command wiring partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 77 tests run, 77 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This wires the profile helper into the CLI layer. It still does not claim the
  installed-Codex smoke harness, server start command, real upstream WebSocket
  tunnel, live OAuth/quota proof, implementation review, or PR readiness.

## T9 Partial Token Export Command Wiring Proof

Timestamp: 2026-06-20T20:20:37Z

Implemented:

- `codex-router-cli/src/lib.rs`
  - `codex-router token export --router-root <path> --shell posix`
  - explicit router-owned root requirement
  - token load through `FileSecretStore` and `LocalRouterTokenService`
  - exactly one `CODEX_ROUTER_TOKEN=...` shell assignment on stdout
  - no surrounding prose and no stderr output on success

TDD proof:

```text
RED: cargo nextest run -p codex-router-cli token_export_command
     failed because token was an unknown command.

GREEN: cargo nextest run -p codex-router-cli token_export_command
       2 token export command tests run, 2 passed, 11 skipped by filter.
```

Full proof after T9 token export command wiring partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 79 tests run, 79 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This exposes token export for later smoke harness use. It does not add live
  OAuth/quota proof, server start/runtime assembly, real upstream WebSocket
  tunnel, installed-Codex smoke, implementation review, or PR readiness.

## T8 Partial Loopback Router Runtime Assembly Proof

Timestamp: 2026-06-20T20:36:54Z

Implemented:

- `codex-router-proxy/src/server.rs`
  - `LoopbackRouterRuntimeConfig`
  - `LoopbackRouterRuntime`
  - runtime-owned loopback listener, SQLite state store, file secret store,
    local auth gate, and HTTP upstream transport
  - bounded `serve_http_connections` that constructs the borrowed
    `RepositoryBackedAccountSelector` and `AuthenticatedHttpProxyService` only
    inside the serve call
  - no router retry, timeout, health, circuit, or provider-gating behavior
- `codex-router-proxy/src/lib.rs`
  - real local-socket runtime test with router-owned temp SQLite/secrets
  - mock upstream transcript proving local auth stripping and selected upstream
    auth injection

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy assembled_loopback_router_runtime
     failed because LoopbackRouterRuntime and LoopbackRouterRuntimeConfig were
     missing from crate::server.

GREEN: cargo nextest run -p codex-router-proxy assembled_loopback_router_runtime
       1 assembled runtime test run, 1 passed, 24 skipped by filter.
```

Full proof after T8 loopback router runtime assembly partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 80 tests run, 80 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This makes the HTTP/SSE router runtime runnable from assembled primitives and
  unblocks later CLI/server and installed-Codex mock smoke work. It still does
  not implement real upstream WebSocket tunneling, the installed-Codex smoke
  harness, live OAuth/quota proof, implementation review, or PR readiness.

## T9 Partial CLI Serve Command Wiring Proof

Timestamp: 2026-06-20T20:48:33Z

Implemented:

- `crates/codex-router-cli/Cargo.toml`
  - added concrete process-edge dependencies on `codex-router-proxy` and
    `codex-router-state`
- `codex-router-cli/src/lib.rs`
  - `codex-router serve`
  - required `--state-db`, `--secret-root`, and `--upstream-base-url`
  - optional `--listen-host`, `--port`, `--now-unix-seconds`,
    `--max-snapshot-age-seconds`, and `--max-connections`
  - local router token loading from the router-owned secret root through
    `LocalRouterTokenService`
  - runtime startup through `LoopbackRouterRuntime`
  - bounded serve path for smoke/test use
  - real local-socket CLI test proving one request forwards through the process
    command path

TDD proof:

```text
RED: cargo nextest run -p codex-router-cli serve_command_starts_runtime
     failed because serve was an unknown command.

GREEN: cargo nextest run -p codex-router-cli serve_command_starts_runtime
       1 serve command test run, 1 passed, 13 skipped by filter.
```

Full proof after T9 CLI serve command wiring partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 81 tests run, 81 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This gives T10 a real CLI path to start the HTTP/SSE router against a mock
  upstream with isolated state and secrets. It still does not prove installed
  Codex, real upstream WebSocket tunneling, live OAuth/quota, implementation
  review, or PR readiness.

## T8 Partial Blocking WebSocket Tunnel Proof

Timestamp: 2026-06-20T21:09:44Z

Implemented:

- `crates/codex-router-proxy/Cargo.toml`
  - added `tungstenite = "0.28.0"` for real blocking WebSocket handshakes and
    frame IO
- `codex-router-proxy/src/websocket.rs`
  - `BlockingWebSocketTunnel`
  - local tungstenite WebSocket accept with handshake header capture
  - composition with existing `AuthenticatedWebSocketRouter`
  - upstream WebSocket connect with sanitized selected-account headers
  - first-frame forwarding unchanged after local auth, account selection, and
    `response.create` validation
  - bounded upstream-to-local frame forwarding for deterministic protocol proof
  - no retry, timeout, health, circuit, or provider-gating policy
- `codex-router-proxy/src/lib.rs`
  - local tungstenite client, router tunnel, and mock tungstenite upstream test
  - mock upstream transcript proves selected upstream auth is injected, local
    token is stripped, and the first frame is byte/text preserved

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy blocking_websocket_tunnel
     failed because BlockingWebSocketTunnel was missing.

GREEN: cargo nextest run -p codex-router-proxy blocking_websocket_tunnel
       1 blocking tunnel test run, 1 passed, 25 skipped by filter.
```

Full proof after T8 blocking WebSocket tunnel partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 82 tests run, 82 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This proves a real local-to-upstream WebSocket tunnel in proxy protocol tests.
  The assembled server/CLI runtime does not yet dispatch WebSocket upgrades
  through this tunnel, and installed-Codex smoke, live OAuth/quota proof,
  implementation review, and PR readiness remain open.

## T8 Partial Runtime WebSocket Dispatch Proof

Timestamp: 2026-06-20T21:20:16Z

Implemented:

- `codex-router-proxy/src/upstream.rs`
  - `UpstreamEndpoint::websocket_url_for_path`, mapping `http` to `ws` and
    `https` to `wss` while preserving Codex route joining
- `codex-router-proxy/src/server.rs`
  - `LoopbackRouterRuntimeConfig::with_max_websocket_upstream_messages`
  - `LoopbackRouterRuntime::serve_protocol_connections`
  - mixed protocol accept loop that peeks the HTTP head only to classify
    WebSocket upgrades
  - WebSocket upgrade dispatch into `BlockingWebSocketTunnel`
  - normal HTTP/SSE dispatch still uses existing `LoopbackHttpAdapter`
- `codex-router-proxy/src/lib.rs`
  - runtime-level WebSocket test using real local tungstenite client, bound
    router listener, router-owned SQLite/secrets, and mock tungstenite upstream

TDD proof:

```text
RED: cargo nextest run -p codex-router-proxy loopback_router_runtime_dispatches_websocket
     failed because LoopbackRouterRuntime::serve_protocol_connections was
     missing.

GREEN: cargo nextest run -p codex-router-proxy loopback_router_runtime_dispatches_websocket
       1 runtime WebSocket dispatch test run, 1 passed, 26 skipped by filter.
```

Full proof after T8 runtime WebSocket dispatch partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 83 tests run, 83 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This wires real WebSocket upgrade handling into the assembled loopback
  runtime. The CLI `serve` command still calls the HTTP-only serve method, so
  the next slice must switch CLI serve to mixed protocol dispatch before T10
  installed-Codex smoke can claim WebSocket coverage.

## T9 Partial CLI Mixed WebSocket Serve Proof

Timestamp: 2026-06-20T21:34:28Z

Implemented:

- `crates/codex-router-cli/Cargo.toml`
  - added `tungstenite` as a dev-dependency for real CLI WebSocket serve tests
- `codex-router-cli/src/lib.rs`
  - `serve` now starts `LoopbackRouterRuntime` with
    `with_max_websocket_upstream_messages`
  - `serve` now calls `serve_protocol_connections`, so the binary path accepts
    both HTTP/SSE and WebSocket upgrade traffic
  - added `--max-websocket-upstream-messages` for deterministic smoke/test
    bounds
  - CLI-level WebSocket test proves the actual `codex-router serve` command
    forwards a WebSocket upgrade through runtime state/secrets to a mock
    upstream

TDD proof:

```text
RED: cargo nextest run -p codex-router-cli serve_command_dispatches_websocket
     failed because --max-websocket-upstream-messages was unknown and the
     command exited before the WebSocket client could connect.

GREEN: cargo nextest run -p codex-router-cli serve_command_dispatches_websocket
       1 CLI WebSocket serve test run, 1 passed, 14 skipped by filter.
```

Full proof after T9 CLI mixed WebSocket serve partial:

```text
cargo fmt --all -- --check: pass
cargo clippy --workspace --all-targets -- -D warnings: pass
cargo nextest run --workspace: 84 tests run, 84 passed, 0 skipped
cargo deny check: advisories ok, bans ok, licenses ok, sources ok
cargo audit: exit 0, scanned Cargo.lock for vulnerabilities
actionlint .github/workflows/ci.yml: pass
forbidden-scope and dependency guard checks: pass
```

Scope note:

- This gives T10 a real binary serve path for WebSocket traffic against a mock
  upstream. Installed-Codex smoke, live OAuth/quota proof, implementation
  review, and PR readiness remain open.

## T10 Installed-Codex Mock Smoke Proof

Timestamp: 2026-06-20T21:55:10Z

Implemented:

- `crates/codex-router-cli/src/profile.rs`
  - changed generated Codex activation from legacy `[profiles.codex-router]`
    in `config.toml` to Codex profile-v2 overlay file
    `codex-router.config.toml`
  - profile provider now includes `name = "codex-router"` because installed
    `codex-cli 0.141.0` rejects an unnamed custom provider
- `crates/codex-router-proxy/src/server.rs`
  - protocol serve loop now isolates per-connection local rejection errors so
    one bad local request cannot stop the long-running router listener
- `crates/codex-router-proxy/src/websocket.rs`
  - blocking tunnel supports sequential `response.create` frames on the same
    local WebSocket, matching installed Codex's prewarm-plus-turn behavior
- `crates/codex-router-test-support/src/installed_codex.rs`
  - installed Codex smoke harness with temp `CODEX_HOME`
  - profile generated through `CodexRouterProfileWriter`
  - local router token generated/exported through `LocalRouterTokenService` and
    `export_token_assignment`
  - router state/secrets seeded through real SQLite and file secret stores
  - mock upstream WebSocket captures handshake headers and first frame
  - redacted transcript written under `tmp/smoke/`
  - hostile no-token smoke proves unauthenticated local WebSocket traffic does
    not connect to upstream
- `tests/smoke/installed_codex_mock.sh`
  - smoke entrypoint now runs both installed-Codex and hostile no-token ignored
    tests

Codex drift corrected:

```text
Installed Codex: codex-cli 0.141.0
Rejected old shape:
  --profile codex-router cannot be used while config.toml contains legacy
  [profiles.codex-router]...
Required profile file:
  $CODEX_HOME/codex-router.config.toml
Required provider field:
  [model_providers.codex-router].name = "codex-router"
Exec approval flag:
  -c approval_policy="never" instead of old --ask-for-approval
```

TDD proof:

```text
RED: cargo test -p codex-router-cli profile_ -- --nocapture
     failed because tests expected the old `config.toml` / `[profiles]`
     profile shape.

GREEN: cargo test -p codex-router-cli profile_ -- --nocapture
       7 profile tests passed.

RED: cargo test -p codex-router-test-support installed_codex_hostile_no_token_smoke_keeps_upstream_empty -- --ignored --nocapture
     failed because `run_hostile_no_token_smoke` did not exist.

GREEN: cargo test -p codex-router-test-support installed_codex_hostile_no_token_smoke_keeps_upstream_empty -- --ignored --nocapture
       1 hostile no-token smoke test passed.
```

Smoke proof:

```text
tests/smoke/installed_codex_mock.sh
exit 0
2 ignored smoke tests passed:
- installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket
- installed_codex_hostile_no_token_smoke_keeps_upstream_empty
```

Latest redacted transcript:

```text
tmp/smoke/installed-codex-mock-26264-1781989282085.json
codex_status: exit status: 0
codex_stdout_contains_smoke_text: true
codex_version: codex-cli 0.141.0
profile_path: .../codex-home/codex-router.config.toml
upstream.handshake_count: 1
upstream.http_probe_count: 0
upstream.authorization_header: <redacted-present>
upstream.local_router_header_present: false
upstream.first_frame_type: response.create
upstream.first_frame_model: gpt-5.5
upstream.first_frame_stream: true
```

Full proof after T10 installed-Codex mock smoke:

```text
cargo fmt --all -- --check: pass, exit 0
cargo clippy --workspace --all-targets -- -D warnings: pass, exit 0
cargo nextest run --workspace: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
cargo deny check: advisories ok, bans ok, licenses ok, sources ok, exit 0
cargo audit: scanned 73 crate dependencies, exit 0
actionlint .github/workflows/ci.yml: pass, exit 0
tests/smoke/installed_codex_mock.sh: 2 ignored smoke tests passed, exit 0
forbidden-scope and dependency guard checks: pass, exit 0
```

Scope note:

- T10 mock installed-Codex smoke is now covered without touching `~/.codex`.
  Live OAuth/quota proof remains approval-gated and unrun. Implementation
  review, PR readiness, and any live account checks remain open.

## T11 Gated Live OAuth And Quota Runbook Proof

Timestamp: 2026-06-20T22:18:00Z

Implemented:

- `docs/testing/live-oauth-quota.md`
  - records the live OAuth/quota gate as `not-run: approval required`
  - names the exact current CLI surface and explicitly states it is not live
    OAuth proof
  - forbids inventing `codex-router live-proof`, `codex-router login`, or
    `codex-router quota` commands before those surfaces are designed,
    implemented, and tested
  - records that this revision has no approved live commands
  - defines the redaction contract for any future approved live proof

Current live gate:

```text
live_oauth_quota_gate: not-run
reason: approval required; no tested live OAuth/quota CLI exists in this revision
next_step_if_required: replan and implement a redacted approval-gated live-proof command before running real accounts
```

Proof:

```text
wc -l docs/testing/live-oauth-quota.md: 116 lines
git diff --check: pass, exit 0
executable-surface fake live command guard: pass, exit 0
runbook required-status markers: present, exit 0
```

Full proof after T11 gated live runbook:

```text
cargo fmt --all -- --check: pass, exit 0
cargo clippy --workspace --all-targets -- -D warnings: pass, exit 0
cargo nextest run --workspace: 85 tests run, 85 passed, 2 ignored smoke tests skipped, exit 0
cargo deny check: advisories ok, bans ok, licenses ok, sources ok, exit 0
cargo audit: scanned 73 crate dependencies, exit 0
actionlint .github/workflows/ci.yml: pass, exit 0
tests/smoke/installed_codex_mock.sh: 2 ignored smoke tests passed, exit 0
```

Scope note:

- No live OAuth, real quota, account rotation, or quota pooling command was run.
  This is the required approval boundary, not a substitute for future approved
  live proof.
