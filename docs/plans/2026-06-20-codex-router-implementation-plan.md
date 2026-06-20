# codex-router Implementation Plan

Date: 2026-06-20
Status: reviewed plan, ready for `shravan-dev-workflow:implementation-execute-plan`
Goal id: 2026-06-20-codex-router

## Goal

Build `codex-router` as a greenfield Rust repo for a narrow local proxy in front
of the real OpenAI Codex CLI. The router owns only local router auth, upstream
OpenAI/ChatGPT OAuth accounts, quota snapshots, account selection, and
byte-preserving forwarding of Codex model-provider traffic.

The implementation must keep Codex behavior in Codex: no Codex launcher, no
Codex install management, no Codex home scanning or repair, no prompt/context
rewriting, no provider health/circuit layer, no timeout/retry policy layer, and
no Prodex multi-provider gateway.

## Source Coverage

Plan-create read these local source artifacts in full:

- `README.md`: 15 lines, read `1-15`.
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md`: 449 lines, read `1-180`, `181-360`, `361-449`.
- `docs/specs/references/2026-06-20-research-evidence.md`: 94 lines, read `1-94`.
- `docs/specs/reviews/2026-06-20-codex-router-spec-review.md`: 66 lines, read `1-66`.
- `tmp/workflow-state/2026-06-20-codex-router/details.md`: 177 lines, read `1-177`.
- `tmp/workflow-state/2026-06-20-codex-router/events.jsonl`: 1 line, parsed as JSONL.

Live evidence checked during planning:

- Codex source: `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex`, commit `d66708232299`, clean status.
- Prodex source: `/Users/shravansunder/Documents/dev/open-source/ai-dev/prodex`, commit `682e442a11b0`, clean status.
- Official Codex manual cache: `/var/folders/4f/697ggy6x26q8kh9qb2js4xnc0000gn/T/openai-docs-cache/codex-manual.md`, previously refreshed on 2026-06-20.
- Local Rust state: `rustup` and `mise` are present, but `rustc`, `cargo`,
  `cargo-nextest`, `cargo-deny`, and `cargo-audit` are not currently on `PATH`.
  Installing or changing these tools mutates host-level state and requires
  explicit approval before implementation runs bootstrap commands.
- Installed Codex CLI: `codex-cli 0.141.0`. `codex debug models` is not a
  runtime/profile command; installed-Codex profile smoke must use runtime
  commands such as `codex --profile codex-router exec ...`.
- Git state: this repo currently has no commits and no configured remote. PR
  readiness remains part of the goal terminal, but remote/repo/PR setup is an
  external precondition that requires explicit authorization before push or
  GitHub creation work.

Codex source references used:

- `codex-rs/model-provider-info/src/lib.rs`: provider ids, `wire_api = "responses"`, `env_http_headers`, `requires_openai_auth`, `supports_websockets`, retry/timeout fields, and env header omission when missing or empty.
- `codex-rs/model-provider/src/provider.rs`: custom providers with `requires_openai_auth = false` return no Codex account state.
- `codex-rs/codex-api/src/endpoint/session.rs`: provider `base_url` plus relative API paths.
- `codex-rs/codex-api/src/endpoint/responses.rs`: `POST responses` SSE path.
- `codex-rs/codex-api/src/endpoint/responses_websocket.rs`: Responses WebSocket headers, `x-codex-turn-state`, `x-models-etag`, and stream transport.
- `codex-rs/codex-api/src/endpoint/models.rs`: `GET models` and standard `ETag`.
- `codex-rs/core/tests/suite/client_websockets.rs`: `response.create` frames and `previous_response_id` in the first request body.
- `codex-rs/core/tests/suite/turn_state.rs`: HTTP and WebSocket turn-state behavior within a turn.
- `codex-rs/Cargo.toml`, `codex-rs/rustfmt.toml`, `codex-rs/rust-toolchain.toml`, `codex-rs/deny.toml`, `codex-rs/.cargo/audit.toml`: Rust 2024, toolchain, formatting, clippy, audit, and deny conventions to adapt in smaller form.

Prodex source-mining references used:

- `crates/prodex-secret-store/src/*`: trait shape, keyring/file backend ideas, private file writes, refresh leases.
- `crates/prodex-runtime-quota/src/*`: quota window summaries, pressure bands, live-vs-persisted snapshot behavior.
- `crates/prodex-app/src/runtime_proxy/responses/*`: previous-response affinity ideas.
- `crates/prodex-app/src/runtime_proxy/websocket/response_tracking/*`: WebSocket precommit buffering and response id observation ideas.

Prodex areas explicitly excluded from implementation:

- `crates/prodex-provider-core`.
- Gateway admin, virtual-key, billing, metrics, guardrail, SCIM, SSO, tenant, Redis, Postgres, OpenAPI, and route-strategy surfaces.
- Runtime health, circuit breaker, overload retry, smart context, prompt rewriting, and Codex home/profile/session repair.

## Decisions Carried Into Implementation

1. WebSocket support is in v1, but selection waits for the first local
   `response.create` frame before opening upstream. The first frame is forwarded
   unchanged after selection.
2. `/v1/responses/compact` is implemented as a compatibility route, with an
   installed-Codex smoke test deciding whether the current installed Codex build
   actually sends it for the `Codex Router` provider.
3. The first secret backend is a hardened file backend behind a trait, plus an
   interface that can add macOS Keychain or 1Password later. This keeps v1
   testable without approving live 1Password or Keychain mutations.
4. Local router auth uses Codex `env_http_headers` with
   `X-Codex-Router-Token = CODEX_ROUTER_TOKEN`. Missing or empty env means Codex
   omits the header, and the router rejects before account selection.
5. All `~/.codex` write paths are optional helper commands, disabled by default,
   preview-first, and require an explicit approval flag.
6. The first implementation uses mocked OAuth/quota/protocol tests for normal
   proof. Real OAuth login, real quota fetch, live account rotation, and live
   quota pooling are gated by explicit user approval.
7. Account, quota, reservation, and affinity metadata use SQLite in v1 through
   a small router-owned state crate with migrations. OAuth tokens remain in the
   secret-store backend, not SQLite.
8. The first implementation includes GitHub Actions CI from T1. Local gates stay
   authoritative when no remote exists yet, but the repo should be PR-ready
   without a later CI-design pass.
9. Audit logging defaults to a router-private file sink under the router-owned
   root with private permissions. Stderr/stdout audit output is opt-in for
   development and must still use the redacted allowlist schema.

## Target Repo Shape

Create a small Rust workspace, not a Codex or Prodex fork:

```text
codex-router/
  Cargo.toml
  rust-toolchain.toml
  rustfmt.toml
  deny.toml
  .cargo/
    audit.toml
  .github/
    workflows/
      ci.yml
  crates/
    codex-router-cli/
    codex-router-core/
    codex-router-auth/
    codex-router-secret-store/
    codex-router-state/
    codex-router-quota/
    codex-router-selection/
    codex-router-proxy/
    codex-router-test-support/
  docs/
    specs/
    plans/
    reviews/
  tests/
    smoke/
```

Crate responsibilities:

- `codex-router-cli`: command parsing, process bootstrap, profile text/dry-run/apply commands, login/logout/enable/disable commands, and server start command. `anyhow` is allowed only here and at process edges.
- `codex-router-core`: ids, config, stable route-kind types, redaction wrappers,
  error types, audit event schema, and shared testable primitives. It must not
  own proxy route tables or protocol parsing.
- `codex-router-auth`: OpenAI/ChatGPT OAuth login state machine, token refresh classification, and account credential models. It depends on secret-store traits, not concrete storage.
- `codex-router-secret-store`: secret trait, hardened file backend, future keychain/1Password adapters, atomic writes, permission checks, symlink refusal, and refresh lease coordination.
- `codex-router-state`: SQLite metadata store, migrations, repository traits,
  and corruption handling for accounts, quota snapshots, reservations, and
  affinity records. It stores no OAuth refresh/access tokens.
- `codex-router-quota`: quota response models, windows, snapshots, background refresh worker, freshness/staleness policy, and persisted snapshot access.
- `codex-router-selection`: account eligibility, weighted deficit round robin, reservations, previous-response affinity, turn-state envelope selection, and precommit rotation decisions.
- `codex-router-proxy`: loopback HTTP/SSE and WebSocket server, route
  classification, route dispatch, auth stripping, upstream auth injection,
  hop-by-hop filtering, forwarding, protocol transcript hooks, and audit
  emission.
- `codex-router-test-support`: mock upstream server, Codex profile fixture, transcript assertions, deterministic clocks, temporary router roots, and installed-Codex smoke harness helpers.

Dependency rules:

- `core` has no dependency on proxy/auth/quota/selection/secret-store.
- `auth` depends on `core` and secret-store traits.
- `state` depends on `core` and may be used by auth/quota/selection through
  repository traits.
- `quota` depends on `core`, auth-owned account/quota-fetch facades, and state
  repository traits. It must not depend on `codex-router-secret-store` or read
  OAuth token material directly.
- `selection` depends on `core`, state repository traits, and quota snapshot
  types, not proxy internals.
- `proxy` depends on `core`, `selection`, `auth` trait facades, `quota` facade, and secret-store interfaces only through higher-level services.
- `cli` wires concrete implementations together.
- `test-support` may depend broadly, but production crates must not depend on it.

## Tooling And Standards

Task 1 must create the toolchain and quality baseline before product code.

Required files:

- `rust-toolchain.toml`
  - channel: `1.95.0`, matching current Codex source snapshot unless plan review chooses a newer verified stable.
  - components: `clippy`, `rustfmt`, `rust-src`.
- `Cargo.toml`
  - `edition = "2024"` at `[workspace.package]`.
  - workspace dependencies centralized.
  - workspace lints copied in small form from Codex:
    - `clippy::await_holding_invalid_type = "deny"`
    - `clippy::await_holding_lock = "deny"`
    - `clippy::expect_used = "deny"`
    - `clippy::unwrap_used = "deny"`
    - `clippy::manual_ok_or = "deny"`
    - `clippy::manual_unwrap_or = "deny"`
    - `clippy::needless_collect = "deny"`
    - `clippy::redundant_clone = "deny"`
    - `clippy::uninlined_format_args = "deny"`
  - no production crate uses `#![allow(clippy::unwrap_used)]` or
    `#![allow(clippy::expect_used)]`.
- `rustfmt.toml`
  - `edition = "2024"`
  - no unstable rustfmt settings on stable toolchain `1.95.0`.
- `deny.toml`
  - allow only the initial license set needed by selected dependencies.
  - no advisory ignores unless the plan-review or implementation step records a
    dependency path and removal condition.
- `.cargo/audit.toml`
  - empty advisory ignore list initially.
- `.github/workflows/ci.yml`
  - mirrors local gates: format, clippy, nextest, deny, audit, actionlint.

Local bootstrap commands for the executor:

```shell
rustup toolchain install 1.95.0 --component clippy,rustfmt,rust-src
rustup component add clippy rustfmt rust-src --toolchain 1.95.0
rustup run 1.95.0 cargo install cargo-nextest --locked
rustup run 1.95.0 cargo install cargo-deny --locked
rustup run 1.95.0 cargo install cargo-audit --locked
brew install actionlint
```

These commands require explicit host-bootstrap approval because they mutate
`~/.rustup`, `~/.cargo`, or the Homebrew prefix. If approval is not granted,
stop before product code and report the setup blocker. Do not weaken the proof
gates. If Homebrew is unavailable, stop and update this plan instead of silently
dropping `actionlint`.

Normal local gates after Task 1 use the resolved cargo binary. If bare `cargo`
is still absent from `PATH`, use `rustup run 1.95.0 cargo` and record that
choice in the proof.

```shell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
cargo audit
```

## Requirements/Proof Matrix

| ID | Requirement / claim | Owning task | Proof owner | Proof layer | Proof gate | Fixture or mock | Expected observation | Source reference | Red/green required | Stale-proof guard |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| R0 | Implementation begins from fresh source provenance, not stale planning evidence. | T0 | parent + implementation-execute-plan | source/provenance | read-only `git -C <repo> status`, `git -C <repo> rev-parse HEAD`, and `git -C <repo> remote -v`; explicit approval gate before `git fetch`, `git pull`, or any external-checkout mutation; Codex manual helper; `codex --version` | local source checkouts and manual cache | current Codex source, Prodex source, official manual cache, installed Codex version, and smoke command shape are recorded before scaffolding | Goal source requirements; plan review parent check | yes | timestamp, commit ids, installed Codex version, command help snippets, and whether external refresh was read-only or approved |
| R1 | Rust workspace is pinned, maintainable, and warning-clean from the first code slice. | T1 | parent + implementation-execute-plan | quality | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo nextest run --workspace`; `cargo deny check`; `cargo audit` | empty workspace then first crates | commands pass; no production unwrap/expect lint escapes | Spec Rust Quality Standards; Codex `Cargo.toml`, `rustfmt.toml`, `rust-toolchain.toml` | yes | record `rustc --version`, `cargo --version`, tool paths, and pinned toolchain |
| R2 | Config accepts only router-owned fields and denies forbidden/unknown fields. | T2 | implementation-execute-plan | unit | `cargo nextest run -p codex-router-core config::` | TOML fixtures | intended fields parse; forbidden fields fail with named errors | Spec Configuration | yes | fixture names include spec date and rejected field |
| R3 | Audit events serialize only an allowlisted redacted schema and default to a private router-root file sink. | T2/T8 | implementation-execute-plan | unit/integration | `cargo nextest run -p codex-router-core audit::`; `cargo nextest run -p codex-router-proxy audit::` | audit snapshots with fake secret markers and temp router root | serialized events include only allowed fields; secret markers absent; default sink path is under private router root with private permissions | Spec Local Auth And Audit Threat Model | yes | snapshot contains canary token/account/email/body strings and temp audit path |
| R4 | Secret wrappers never expose raw values through debug, display, serialization, panic, or logs. | T2/T3 | implementation-execute-plan | unit | `cargo nextest run -p codex-router-core redaction::`; `cargo nextest run -p codex-router-secret-store secret_value::` | fake tokens and panic/log capture | raw secret canary never appears | Spec Secret Storage and Security Requirements | yes | canary string searched in captured output |
| R4A | Config rejects non-loopback listener values and proxy binds loopback only in v1. | T2/T8 | implementation-execute-plan | unit/integration | `cargo nextest run -p codex-router-core config::listen_address`; `cargo nextest run -p codex-router-proxy bind::loopback_only` | config fixtures and local bind test | `127.0.0.1`, `localhost`, and `::1` are accepted; `0.0.0.0`, `::`, and non-loopback addresses reject before server start | Spec Security Requirements and Configuration | yes | fixture includes rejected address and error code |
| R5 | Proxy-owned route classifier supports required Codex routes and fails closed on unknown and Realtime/WebRTC routes before account selection. | T8 | implementation-execute-plan | unit/protocol | `cargo nextest run -p codex-router-proxy routes::`; protocol tests in proxy | route table fixtures | required routes classify; `/v1/realtime`, `/v1/realtime/calls`, unknown paths reject before account selection spy fires | Spec Supported Codex Traffic and Compatibility Matrix | yes | Codex source commit and route list recorded in test fixture comments |
| R6 | Local bearer auth rejects missing/bad token before any account selection or upstream open. | T4/T8 | implementation-execute-plan | integration/protocol/smoke | `cargo nextest run -p codex-router-proxy local_auth::`; hostile smoke | mock selector and mock upstream | unauthorized request returns local error; selector/upstream call count is zero | Spec Local Auth; Codex `env_http_headers` behavior | yes | test runs with env missing, empty, wrong, and correct |
| R7 | Local token rotation invalidates old HTTP/WS auth and closes old-token WebSockets without leaking secrets. | T4/T8 | implementation-execute-plan | integration/protocol | `cargo nextest run -p codex-router-proxy token_rotation::` | mock WebSocket client | old token rejected; old-token WS closed with redacted reason; committed HTTP/SSE can finish | Spec Local Auth And Audit Threat Model | yes | close reason and logs searched for token canary |
| R8 | HTTP/SSE proxy preserves method, path, query, body, status, headers, and event order while stripping local auth and injecting selected upstream auth once. | T8 | implementation-execute-plan | protocol | `cargo nextest run -p codex-router-proxy http_sse_preservation::` | mock upstream transcript | transcript matches input except allowed header changes; upstream auth appears once | Spec Supported Traffic; Codex `responses.rs` | yes | transcript fixture includes body/header canaries and event order ids |
| R9 | `/v1/models` preserves standard `ETag` semantics and does not invent `x-models-etag` for HTTP. | T8 | implementation-execute-plan | protocol | `cargo nextest run -p codex-router-proxy models::` | mock models endpoint | `ETag` passes through; body preserved; local auth stripped | Spec Supported Traffic; Codex `models.rs` | yes | fixture points to Codex commit |
| R10 | WebSocket routing waits for the first local `response.create` frame, reads bounded routing metadata, then opens upstream and forwards the first frame unchanged. | T8 | implementation-execute-plan | protocol | `cargo nextest run -p codex-router-proxy websocket_first_frame::` | mock WS client/upstream transcript | no upstream connection before first frame; selected account matches metadata; first frame byte/text value forwarded unchanged | Spec Routing Granularity; Codex `client_websockets.rs` | yes | fixture includes `previous_response_id` in first frame |
| R11 | WebSocket account selection is connection-scoped in v1, not per-message switching. | T6/T8 | implementation-execute-plan | unit/protocol | `cargo nextest run -p codex-router-selection websocket_affinity::`; proxy transcript test | multi-frame WS transcript | one account reservation for connection; later frames do not switch accounts | Spec Product Laws and Routing Granularity | yes | transcript contains later conflicting hints that must be ignored |
| R11A | WebSocket upstream handshake strips local auth and client-supplied upstream auth before injecting router-selected upstream auth exactly once. | T8/T10 | implementation-execute-plan | protocol/smoke | `cargo nextest run -p codex-router-proxy websocket_handshake_headers::`; installed smoke transcript | mock WS client/upstream handshake transcript | upstream handshake has no `X-Codex-Router-Token`, no client-supplied `Authorization` or cookie auth, hop-by-hop headers stripped, and selected upstream auth injected exactly once | Spec Supported Traffic and Security Requirements | yes | transcript includes local token and hostile auth canaries |
| R11B | WebSocket pre-selection handling is bounded for missing, binary, malformed, non-`response.create`, and oversized first frames. | T8 | implementation-execute-plan | protocol | `cargo nextest run -p codex-router-proxy websocket_preselection_guards::` | mock WS client with hostile first-frame cases | router closes locally with redacted reasons; no upstream connection opens; max first-frame bytes and bounded pre-selection wait are enforced | Spec Routing Granularity and Security Requirements | yes | deterministic timeout/event wait and oversized-frame fixture |
| R12 | `x-codex-turn-state` is a signed router envelope carrying router pin plus upstream token, rejects replay, and forwards only upstream token when needed. | T6/T8 | implementation-execute-plan | unit/protocol | `cargo nextest run -p codex-router-selection turn_state::`; proxy header/frame test | deterministic signing key and clocks | valid envelope pins account; expired/wrong-session/wrong-key replay rejects; upstream sees only upstream token | Spec Routing Granularity; Codex `turn_state.rs` | yes | deterministic clock and key id recorded |
| R13 | Previous-response affinity prefers owning account and fails clearly if the owning account is disabled or unauthenticated. | T6 | implementation-execute-plan | unit/protocol | `cargo nextest run -p codex-router-selection previous_response::` | affinity store fixture | owner selected when eligible; disabled/unauth owner returns clear local failure, not silent different account | Spec Routing Granularity; Prodex previous-response source-mining | yes | fixture includes owner account id and disabled reason |
| R14 | Quota snapshots refresh in background; request-time routing reads existing SQLite snapshots and does not block accept loop on broad refresh. | T5/T7 | implementation-execute-plan | unit/integration | `cargo nextest run -p codex-router-state`; `cargo nextest run -p codex-router-quota refresh::`; `cargo nextest run -p codex-router-selection scoring::` | deterministic clock, temp SQLite database, mock quota service | migrations apply; background worker updates snapshots; selector uses fresh > persisted > unknown with staleness penalty | Spec Account And Quota Model; Prodex quota source-mining | yes | fixture clock, migration version, schema version, and temp database path recorded |
| R15 | Weighted deficit round robin balances by quota headroom and reservations, not naive modulo. | T6 | implementation-execute-plan | unit | `cargo nextest run -p codex-router-selection weighted_deficit::` | account set fixtures | high-headroom accounts receive more turns; reservations reduce immediate headroom; stale snapshots penalized | Spec Account And Quota Model | yes | deterministic selection seed and expected distribution checked |
| R16 | Precommit rotation happens only for explicit account/auth/quota rejection, not for 5xx, overload, timeout, DNS, reset, cancellation, or post-commit stream failure. | T6/T8 | implementation-execute-plan | unit/protocol | `cargo nextest run -p codex-router-selection precommit_rotation::`; proxy failure transcript tests | mock upstream statuses/errors | auth/quota before commit can rotate; forbidden failure classes do not rotate | Spec Product Laws and Protocol Proof | yes | each failure class has a named fixture |
| R17 | Hardened file secret backend uses private permissions, atomic temp-write/rename, parent validation, and symlink refusal. | T3 | implementation-execute-plan | integration | `cargo nextest run -p codex-router-secret-store file_backend::` | tempdir with symlink and permission fixtures | private mode; symlink refused; temp file atomic path used; parent checked | Spec Secret Storage; Prodex secret-store source-mining | yes | temp root path captured; no home path used |
| R18 | Refresh uses single-flight leases and stale-lock recovery to prevent refresh storms. | T3/T5 | implementation-execute-plan | integration | `cargo nextest run -p codex-router-secret-store refresh_lease::`; quota/auth integration tests | concurrent tasks and fake clock | one owner refreshes; followers reuse result; stale lock recovers | Spec Secret Storage; Prodex refresh lease source-mining | yes | deterministic fake clock, no wall-clock sleep |
| R18A | Corrupt or partially persisted auth/quota/account/affinity state fails closed for affected accounts without taking down healthy accounts. | T3/T5/T6/T7 | implementation-execute-plan | integration | `cargo nextest run -p codex-router-state corruption::`; `cargo nextest run -p codex-router-auth corruption::`; `cargo nextest run -p codex-router-quota corruption::` | temp SQLite database, truncated records, bad migration version, partial refresh-result fixtures | corrupt account disabled or isolated with redacted diagnostic; healthy accounts remain eligible; no refresh loop | Spec Security Requirements and Rollback/Recovery | yes | corrupt fixture name, schema version, and redaction canary recorded |
| R19 | Profile helper prints exact custom-provider profile text, shell-safe token export, and dry-run diffs without mutating `~/.codex`; apply requires explicit approval and writes only named profile file. | T4/T9 | implementation-execute-plan | integration/smoke | `cargo nextest run -p codex-router-cli profile::`; `cargo nextest run -p codex-router-cli token_export::`; installed-Codex smoke setup | temp `CODEX_HOME`, temp router root, generated local token | print/dry-run no mutation; profile contains `model_provider = "codex-router"`, `[model_providers.codex-router]`, `base_url`, `wire_api = "responses"`, `requires_openai_auth = false`, `supports_websockets = true`, and `env_http_headers = { "X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN" }`; export command emits exactly one shell-safe assignment for `CODEX_ROUTER_TOKEN`; apply without flag fails; apply with flag writes only `<profile>.config.toml` in temp home | Spec Activation Model and Local Auth | yes | temp `CODEX_HOME`; real `~/.codex` untouched; shell-escaping fixture includes quote/newline canaries and no token in logs |
| R20 | Installed Codex can run through a helper-rendered router profile and helper-rendered token env against a mock upstream with isolated config. | T10 | parent + implementation-execute-plan | smoke | `tests/smoke/installed_codex_mock.sh` or cargo smoke harness command defined in T10 | temp `CODEX_HOME`, mock upstream, helper-rendered local token env | captures Codex version/profile; smoke consumes T9 profile output and T4/T9 token export output rather than hand-built fixtures; `codex --profile codex-router exec ...` exercises `/v1/responses`; mock transcript proves any models/protocol probes required by the smoke harness; HTTP/SSE and WS paths are exercised | Spec Smoke Proof; official Codex manual profiles; installed Codex `0.141.0` help output | yes | records `codex --version`, profile file, command help snippets, mock transcript, no token printed |
| R21 | Hostile local request without router token never opens upstream. | T10 | parent + implementation-execute-plan | smoke | hostile-token smoke command defined in T10 | local request client and mock upstream | unauthorized local request returns local auth error; upstream transcript empty | Spec Smoke Proof | yes | upstream transcript count asserted zero |
| R22 | Live OAuth login, real quota fetch, real account rotation, and live quota pooling are never run without explicit user approval. | T11 | parent orchestrator | gated live | live commands are documented but skipped unless approved | real accounts only after approval | without approval, live gate reports `not-run: approval required`; with approval, redacted evidence only | Spec Gated Live Proof | no until approval | approval transcript and redacted account labels |
| R23 | Product non-goals remain absent from config and code. | T1-T11 | plan-review-swarm + implementation-review-swarm | review/quality | `rg` guard commands plus implementation review | repo search | no Codex home scan/repair, no provider-core/gateway imports, no smart-context/prompt rewriting, no health/circuit policy | Spec Product Laws, Configuration, Source-Mining Policy | yes | exact `rg` patterns captured after implementation |
| R24 | PR readiness is proven and not merged without authorization. | T12 | implementation-pr-wrapup | PR/release | PR checks, review threads, mergeability, current diff | GitHub PR | checks green or scoped blockers reported; review threads handled; PR ready; no merge | Goal terminal condition | yes | fresh PR state timestamp |

## Task Sequence

### T0. Source Provenance And Smoke Command Preflight

Write surfaces:

- `docs/wip/implementation-provenance.md` or a timestamped execution note under
  `tmp/` during implementation.

Implementation notes:

- Verify the current Codex source checkout before scaffolding using read-only
  git commands unless an external-checkout refresh is explicitly approved.
- Verify the current Prodex source checkout before source-mining using read-only
  git commands unless an external-checkout refresh is explicitly approved.
- Run the official Codex manual helper and record the cache path/timestamp.
- Record installed Codex version with `codex --version`.
- Verify smoke command syntax with:
  - `codex debug models --help`
  - `codex --profile codex-router exec --help`
- Record that `codex debug models --profile ...` and
  `codex --profile ... debug models` are not valid profile smoke commands in
  installed Codex `0.141.0`; do not use them as installed-Codex smoke gates.
- If Codex source/manual/installed CLI semantics drift from the spec or plan,
  stop and update the spec/plan before implementation.
- Check required host tools with
  `command -v rustup rustc cargo cargo-nextest cargo-deny cargo-audit actionlint`.
- External Codex and Prodex source checkout verification is read-only by
  default: use `git status`, `git rev-parse`, and `git remote -v` first. Do not
  run `git pull`, `git fetch`, or any source-checkout mutation unless explicitly
  approved for provenance refresh.
- If `rustc`, `cargo`, `cargo-nextest`, `cargo-deny`, `cargo-audit`, or
  `actionlint` are missing, stop before T1 and request explicit approval for
  host bootstrap. Do not run global `rustup` or `cargo install` commands as an
  automatic repo implementation step. Do not run `brew install actionlint`
  without that same approval.

Proof:

- Provenance note contains commit ids, manual cache path, installed Codex
  version, and smoke command help snippets.

### T1. Rust Workspace And Quality Baseline

Write surfaces:

- `Cargo.toml`
- `rust-toolchain.toml`
- `rustfmt.toml`
- `deny.toml`
- `.cargo/audit.toml`
- `.github/workflows/ci.yml`
- `.gitignore`
- initial crate directories with empty `lib.rs` or minimal bootstrap code

Implementation notes:

- Use `rustup` to install the pinned toolchain before writing code if `rustc`
  and `cargo` are still absent from `PATH`.
- Keep dependencies minimal:
  - runtime: `tokio`, `axum`, `hyper`/`reqwest` as selected in implementation,
    `tokio-tungstenite`, `serde`, `serde_json`, `toml`, `thiserror`, `tracing`,
    `zeroize`, `secrecy` or local secret wrappers, `uuid`, `time`, `hmac`,
    `sha2`, `base64`.
  - CLI: `clap`, `anyhow`, `tracing-subscriber`.
  - tests: `tempfile`, `wiremock` or local mock servers, `insta` only if plan
    review accepts snapshots for audit/schema proof.
- Use `thiserror` in library crates. Restrict `anyhow` to CLI/bootstrap.

Proof:

- Bootstrap commands complete or report exact setup blocker.
- Quality gates pass on the empty/minimal workspace.

Split/replan trigger:

- If the pinned Codex toolchain `1.95.0` is unavailable through `rustup`, plan
  review must choose a verified stable toolchain and update the plan before
  code execution continues.

### T2. Core Types, Config, Redaction, And Audit Schema

Write surfaces:

- `crates/codex-router-core/src/config.rs`
- `crates/codex-router-core/src/ids.rs`
- `crates/codex-router-core/src/redaction.rs`
- `crates/codex-router-core/src/audit.rs`
- `crates/codex-router-core/src/error.rs`
- unit tests under the same crate

Implementation notes:

- Config uses deny-unknown-fields.
- Forbidden config categories have explicit negative tests.
- Listen address validation accepts loopback only in v1 and rejects `0.0.0.0`,
  `::`, LAN, and other non-loopback values before server start.
- Important ids are newtypes: `AccountId`, `RouteId`, `ReservationId`,
  `AffinityKey`, `RequestId`, `TokenGeneration`.
- Secret-bearing types do not implement raw `Debug`, `Display`, or `Serialize`.
- Audit events use an enum/struct schema with no arbitrary JSON extension field.
- Default audit sink is a private file under the router-owned root. T2 defines
  the config and schema; T8 proves proxy emission and file permissions.

Proof:

- Unit tests for config accept/deny behavior, audit allowlist, redaction, and
  secret canary absence.

### T3. Secret Store And Refresh Lease

Write surfaces:

- `crates/codex-router-secret-store/src/lib.rs`
- `crates/codex-router-secret-store/src/file_backend.rs`
- `crates/codex-router-secret-store/src/refresh_lease.rs`
- `crates/codex-router-secret-store/src/model.rs`

Implementation notes:

- Start with hardened file backend behind a trait.
- Root path is router-owned, private, and never `~/.codex`.
- Validate parent directories and reject symlinks.
- Use temp write plus atomic rename.
- Add a refresh lease with deterministic test clock; do not use wall-clock sleeps
  in tests.
- Add corruption fixture helpers that T5/T6/T7 can reuse for SQLite metadata and
  partial refresh-result tests.

Proof:

- Integration tests for permissions, atomic behavior, symlink refusal, parent
  validation, owner/follower lease behavior, and stale-lock recovery.

### T4. Local Router Auth

Write surfaces:

- `crates/codex-router-core/src/local_auth.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-cli/src/token.rs`

Implementation notes:

- Generate a local router bearer token through the real secret-store interface
  from T3.
- Provide an explicit shell-safe export command for activation, for example
  `codex-router token export --shell posix`, that emits exactly one assignment
  for `CODEX_ROUTER_TOKEN` and no surrounding prose.
- Store token generation metadata in router-owned state.
- Reject missing, empty, wrong, and old tokens before selection.
- Do not print token in doctor output.
- The export command is the only CLI path allowed to reveal the local router
  token. Its output must be shell-escaped, never logged by the router, and tested
  with quote/newline/token-canary fixtures.
- Token rotation closes old-token WebSockets and lets already committed HTTP/SSE
  responses finish.
- Do not introduce any temporary in-memory or ad hoc file token store.

Proof:

- Unit tests for token parsing, redaction, and shell-safe export escaping.
- Integration tests for missing/bad/old token behavior and WS close behavior
  using a temp router root and the T3 secret backend.

### T5. OAuth Account Store, SQLite Metadata, And Quota Snapshot Model

Write surfaces:

- `crates/codex-router-auth/src/*`
- `crates/codex-router-state/src/*`
- `crates/codex-router-quota/src/*`
- router-owned SQLite migrations and schema docs

Implementation notes:

- OAuth login and refresh must be real code behind provider traits and must be
  proven against mock authorization/token/quota endpoints before any live
  account approval is requested.
- Live OAuth accounts are gated, but mock OAuth is not optional.
- Auth owns the credential lifecycle, expiry classification, refresh
  classification, token exchange, refresh-result persistence, and auth error
  taxonomy. Quota asks auth for an authenticated quota fetch; quota never reads
  secret-store material directly.
- Generic multi-provider auth abstractions are forbidden in v1.
- SQLite is the v1 metadata store for account registry metadata, quota
  snapshots, reservations, and affinity records.
- Migrations are part of the implementation. Corrupt metadata fails closed for
  the affected account or operation with redacted diagnostics.
- Files are acceptable only for secret material and small deterministic
  non-database fixtures.
- Quota snapshots include source, observed time, age, route bands, remaining
  headroom, reset hints, and stale penalty status.

Proof:

- Unit tests for expiry, refresh-needed, quota windows, snapshot freshness,
  migration versioning, and persisted fallback.
- Integration tests for SQLite migration/open/corruption handling and background
  refresh using mock OAuth/token/quota services.

### T6. Selection And Routing State Machine

Write surfaces:

- `crates/codex-router-selection/src/eligibility.rs`
- `crates/codex-router-selection/src/weighted_deficit.rs`
- `crates/codex-router-selection/src/reservation.rs`
- `crates/codex-router-selection/src/affinity.rs`
- `crates/codex-router-selection/src/turn_state.rs`
- `crates/codex-router-selection/src/precommit.rs`

Implementation notes:

- Use weighted deficit round robin over eligible accounts.
- Penalize stale/unknown quota when known healthy accounts exist.
- Reservations reduce immediate headroom.
- Previous-response affinity overrides balance only when the owner is eligible.
- Signed turn-state envelope carries account pin and optional upstream token.
- Precommit rotation policy is explicit and narrow.

Proof:

- Unit tests for each selector branch and rotation/fail/no-rotate class.
- Property-style table tests are acceptable if deterministic and readable.

### T7. Background Refresh Runtime

Write surfaces:

- `crates/codex-router-quota/src/worker.rs`
- `crates/codex-router-auth/src/refresh_worker.rs`
- `crates/codex-router-cli/src/doctor.rs`

Implementation notes:

- Server start uses existing snapshots immediately and schedules refresh.
- Request path never blocks on broad all-account refresh.
- Doctor reports stale/missing state without secrets.

Proof:

- Integration test with bounded event waits, not wall-clock sleeps.
- Doctor output tests with redaction canaries.

### T7.5. Contracts Frozen Before Proxy Integration

Write surfaces:

- contract traits or DTOs in `codex-router-core`, `codex-router-auth`,
  `codex-router-quota`, `codex-router-selection`, and `codex-router-state`.

Implementation notes:

- Freeze these interfaces before T8 starts:
  - `AuthenticatedQuotaClient`: auth-owned facade used by quota refresh.
  - `AccountStateRepository`: state-owned account metadata API.
  - `QuotaSnapshotRepository`: state-owned quota snapshot API.
  - `AffinityRepository`: state-owned previous-response and turn-state pin API.
  - `SelectionDecision`: account id, reservation id, affinity reason, and audit
    reason returned by selection to proxy.
  - `ReservationHandle`: create/finalize/release lifecycle used by proxy.
  - `TurnStateEnvelopeCodec`: encode/decode API that forwards only upstream
    token material when needed.
  - `PrecommitFailureClassifier`: selection-owned decision input for explicit
    auth/quota rotation, with proxy-owned raw response parsing converted into
    bounded classifier inputs.
- Proxy must compile against mocks/fakes for auth/quota/selection/state before
  real upstream wiring.

Proof:

- Unit tests for each contract DTO and error mapping.
- Dependency guard proves quota does not import secret-store and production
  crates do not depend on test-support.

### T8. HTTP/SSE And WebSocket Proxy

Write surfaces:

- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/routes.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/headers.rs`
- `crates/codex-router-proxy/src/upstream.rs`
- `crates/codex-router-test-support/src/mock_upstream.rs`
- `crates/codex-router-test-support/src/transcript.rs`

Implementation notes:

- Bind loopback only in v1.
- Support required routes:
  - `POST /v1/responses`
  - WebSocket upgrade on `/v1/responses`
  - `GET /v1/models`
  - `POST /v1/memories/trace_summarize`
  - `POST /v1/responses/compact`
- Fail closed before selection on unsupported paths, including Realtime/WebRTC.
- Preserve unknown Codex request fields.
- Strip local router token and hop-by-hop headers.
- Inject selected upstream auth exactly once.
- Do not add router retry/timeout/health/circuit behavior beyond server/runtime
  mechanics needed to forward.
- For WebSocket, accept local upgrade after auth, wait for the first text
  `response.create` frame, select account, open upstream, forward the first
  frame unchanged, then pin the connection.
- WebSocket upstream handshakes must strip the local router token, hop-by-hop
  headers, and any client-supplied upstream `Authorization` or cookie auth before
  injecting the selected upstream account auth exactly once.
- WebSocket pre-selection handling must define and test max first-frame bytes,
  accepted first-frame type, malformed/non-`response.create` close behavior, and
  a bounded pre-selection wait. These are local resource guards, not a generic
  upstream timeout/retry policy layer.

Proof:

- Protocol transcript tests for HTTP/SSE preservation, models `ETag`, compact,
  memories, unsupported paths, local auth, upstream auth injection, failure
  class rotation rules, WebSocket first-frame routing, WebSocket handshake
  headers, and WebSocket pre-selection hostile cases.

### T9. Codex Profile Helper

Write surfaces:

- `crates/codex-router-cli/src/profile.rs`
- integration tests using temp `CODEX_HOME`

Implementation notes:

- `codex-router profile print` prints the intended profile.
- `codex-router profile doctor` reports env token presence without token value.
- `codex-router profile write --dry-run` previews exact target file and diff.
- `codex-router profile write --approve-codex-home-write` is the only command
  allowed to write a profile file.
- Rendered profile content must include:
  - `[profiles.codex-router]`
  - `model_provider = "codex-router"`
  - `[model_providers.codex-router]`
  - `base_url = "http://127.0.0.1:<port>/v1"`
  - `wire_api = "responses"`
  - `requires_openai_auth = false`
  - `supports_websockets = true`
  - `env_http_headers = { "X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN" }`
- Real `~/.codex` is not touched in tests.

Proof:

- Integration tests for no-mutation default, dry-run, approval requirement, and
  exact temp profile write.
- Snapshot or structured TOML parse tests for exact custom-provider content.
- Integration tests that combine the T4 token export command and the rendered
  profile with a temp `CODEX_HOME`.

### T10. Installed-Codex Mock Smoke

Write surfaces:

- `tests/smoke/installed_codex_mock.sh` or an equivalent cargo-driven smoke
  harness.
- `crates/codex-router-test-support/src/installed_codex.rs`

Implementation notes:

- Use a temp `CODEX_HOME` or equivalent isolated profile fixture.
- Generate the temp Codex profile through the T9 profile helper. Do not handwrite
  the custom-provider fixture in the smoke harness.
- Generate local router token env through the T4/T9 token export path and consume
  that helper output in the smoke harness. Do not hand-inject
  `CODEX_ROUTER_TOKEN` from bespoke smoke code.
- Start router with mock upstream.
- Capture `codex --version`.
- Run `codex --profile codex-router exec --cd <tmp-workdir> --ask-for-approval never --sandbox read-only "<prompt>"` against mock router to exercise `/v1/responses`.
- If the smoke harness needs a `/v1/models` probe, use either the router's
  protocol mock tests or a debug command with explicit `-c` overrides after
  verifying that exact command against the installed Codex binary. Do not use
  `--profile` with `debug models`.
- With the v1 profile declaring `supports_websockets = true`, installed-Codex
  smoke must assert at least one real WebSocket handshake and capture the first
  `response.create` frame. If the current installed Codex build does not choose
  WebSocket despite that profile, stop and replan because the source assumption
  is wrong.
- Assert mock transcript for header stripping, upstream auth injection, body and
  frame preservation.
- Add a hostile local no-token smoke where upstream transcript remains empty.

Proof:

- Smoke command exits zero and writes redacted transcript artifact under
  `tmp/smoke/` or test output directory.

### T11. Gated Live OAuth And Quota Proof

Write surfaces:

- `docs/wip/live-proof-runbook.md` or `docs/testing/live-oauth-quota.md`
- optional ignored local fixture templates

Implementation notes:

- Do not run live tests without explicit approval in the transcript.
- Live proof must redact account labels, tokens, request/response bodies,
  prompts, memory traces, and tool arguments.
- Live tests prove login, quota fetch, account rotation, and quota pooling only.
- T11 is runbook-only unless plan review explicitly adds a CLI live-proof
  subcommand. The runbook must name exact approved commands that can be invoked
  verbatim; do not reference a `codex-router live-proof` command unless T11 also
  adds and tests that CLI surface.

Proof:

- Without approval: plan/implementation reports the live gate as not run,
  approval required.
- With approval: redacted evidence and exact commands are captured.

### T12. Review, PR, And Readiness

Write surfaces:

- review fixes only after `implementation-review-swarm`.
- PR metadata only after implementation proof.

Implementation notes:

- Run `plan-review-swarm` before implementation.
- Run `implementation-execute-plan` after reviewed plan.
- Run `implementation-review-swarm` after implementation proof.
- Run `implementation-pr-wrapup` to open/update/prove PR readiness.
- Do not merge without explicit authorization.

Proof:

- Implementation proof gates pass or scoped blockers are reported.
- Review findings addressed or explicitly rejected.
- PR checks/review threads/mergeability freshly reported.

## Validation Gates By Layer

Source/provenance:

```shell
codex --version
codex debug models --help
codex --profile codex-router exec --help
```

Unit:

```shell
cargo nextest run -p codex-router-core
cargo nextest run -p codex-router-state
cargo nextest run -p codex-router-selection
cargo nextest run -p codex-router-quota
cargo nextest run -p codex-router-auth
```

Integration:

```shell
cargo nextest run -p codex-router-secret-store
cargo nextest run -p codex-router-proxy
cargo nextest run -p codex-router-cli
```

Protocol:

```shell
cargo nextest run -p codex-router-proxy http_sse::
cargo nextest run -p codex-router-proxy websocket::
cargo nextest run -p codex-router-proxy protocol::
```

Smoke:

```shell
tests/smoke/installed_codex_mock.sh
```

Quality:

```shell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
cargo audit
```

Guard searches:

```shell
rg -n "prodex-provider-core|prodex_provider_core|gateway_admin|virtual_key|route_strategy|smart_context|shared_codex_home|PRODEX_SHARED_CODEX_HOME|session_repair|codex_home_symlink|codex_home_repair" crates Cargo.toml rust-toolchain.toml rustfmt.toml .cargo .github tests
rg -n "unwrap\\(|expect\\(" crates
! cargo tree -p codex-router-quota -e normal,build | rg "codex-router-secret-store"
! cargo metadata --format-version 1 --no-deps | jq -e '.packages[] | select(.name != "codex-router-test-support") | .dependencies[]? | select(.name == "codex-router-test-support")'
actionlint .github/workflows/ci.yml
```

The `unwrap`/`expect` guard search may find test-only assertions. Production
uses clippy denies; any test allowances must be crate-local, explicit, and
reviewed.

Gated live:

```shell
# Not run unless explicitly approved:
docs/testing/live-oauth-quota.md must provide the exact redacted live commands.
```

PR readiness:

```shell
gh pr status
gh pr checks
gh pr view --json mergeStateStatus,reviewDecision,statusCheckRollup
```

## Security Assumptions

- Threat model excludes a fully compromised same-user account.
- Loopback binding is not authentication.
- Same-user hostile local HTTP requests are in scope and must be rejected by the
  local bearer token.
- Secrets include OAuth refresh tokens, access tokens, local router token,
  account labels, quota snapshots, affinity pins, audit logs, and transient
  Codex payloads.
- The proxy must not inspect prompts, tool arguments, images, files, or memory
  traces for routing decisions.
- The audit/event system must be structured enough to prove routing decisions
  without storing arbitrary request/response details.

## Rollback And Recovery

- T1 rollback: remove generated Rust workspace files before product code exists.
- T2-T9 rollback: because this is a greenfield repo, revert the scoped task diff
  or reset only the new task branch if explicitly authorized.
- Runtime recovery: router state must tolerate corrupt account/quota records by
  failing closed for that account and keeping other accounts eligible when safe.
- Secret-store recovery: ambiguous credential corruption fails closed and emits
  redacted diagnostics; it must not try to repair or import Codex `auth.json`.
- Profile helper recovery: dry-run is default; approved profile writes target
  only the named profile file and should be easy to remove manually.

## Risks And Replan Triggers

- If current Codex changes provider profile semantics or route paths, update the
  spec and plan before implementation continues.
- If installed Codex does not exercise WebSocket for the custom provider despite
  `supports_websockets = true`, stop and replan before claiming smoke proof.
- If `/v1/responses/compact` is not used by installed Codex for this provider,
  keep route compatibility tests but mark installed compact smoke conditional.
- If hardened file store cannot meet symlink/permission guarantees portably,
  split keychain/1Password backend selection into a design update before live
  OAuth work.
- If SQLite is selected for account/quota/affinity state, add migration and
  corruption tests before proxy integration depends on it.
- If live OAuth/quota proof becomes necessary for implementation confidence,
  stop for explicit approval rather than weakening the gate.

## Plan Review Decisions

1. Pin `rust-toolchain.toml` to Codex's current `1.95.0`. If rustup cannot
   install it, stop before product code and update the plan rather than silently
   choosing another toolchain.
2. Use SQLite for account/quota/reservation/affinity metadata in v1, with
   migrations and corruption tests. Store OAuth tokens only in the secret store.
3. Use the hardened file secret backend as the first concrete backend because it
   is deterministic and can be proven locally. Keep macOS Keychain and
   1Password as future adapters behind the same trait.
4. Installed-Codex smoke must use the real runtime command shape:
   `codex --profile codex-router exec ...`. `codex debug models` does not accept
   profiles in installed Codex `0.141.0`; any `/v1/models` debug probe must use
   verified `-c` overrides or stay as a protocol/mock test. If installed Codex
   does not exercise WebSocket for the custom provider, stop and replan.
5. Include GitHub Actions in T1. Local commands remain the proof source until a
   remote PR exists.
6. Host toolchain/bootstrap commands are not ordinary repo-local execution.
   Missing `rustc`, `cargo`, `cargo-nextest`, `cargo-deny`, `cargo-audit`, or
   `actionlint` requires an explicit host-bootstrap approval checkpoint.
7. If no Git remote or GitHub repo exists by T12, the implementation is not
   terminal-complete. Stop at PR wrap-up, report the missing remote/PR as an
   external authorization blocker, and do not mark the goal complete.

## Phase Footer

phase_result: complete
evidence: this reviewed plan, plan-review report, source coverage above,
live Codex/Prodex source checks, local Rust toolchain check, plan artifact
validation, and stale-command validation
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: The implementation plan has been adversarially
reviewed and revised; the next unproven lifecycle gate is implementation
execution starting with T0 source provenance and host-bootstrap approval
detection.
