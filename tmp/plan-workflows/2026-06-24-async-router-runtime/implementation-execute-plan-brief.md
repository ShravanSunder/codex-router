# Implementation Execute Plan Brief

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Workflow: `shravan-dev-workflow:implementation-execute-plan`
Controller: Codex

## Current State

- Branch: `main`
- Starting HEAD: `9fbd57e`
- Planning checkpoint: committed and pushed as
  `docs: add reviewed async runtime execution plan`
- Working tree at entry: only unrelated resume-picker scratch artifacts are
  untracked under `tmp/spec-workflows/2026-06-24-codex-router-resume-picker/`
- Official transition source:
  `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`
- Latest official next workflow:
  `shravan-dev-workflow:implementation-execute-plan`

## Required Artifacts Loaded

- Plan:
  `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
  - line count: 868
  - controller coverage: lines 1-868 loaded in five chunks
- Spec:
  `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
  - line count: 759
- State details:
  `tmp/workflow-state/2026-06-24-async-router-runtime/details.md`
- Transition log:
  `tmp/workflow-state/2026-06-24-async-router-runtime/events.jsonl`

## Live Repo Validation Snapshot

Observed release runtime paths before implementation:

- `crates/codex-router-cli/src/lib.rs`
  - `CliCommand::Serve` starts `LoopbackRouterRuntime` and then calls
    `serve_protocol_connections`.
- `crates/codex-router-proxy/src/server.rs`
  - release serve path uses `std::net::TcpListener`, `TcpStream`,
    `std::thread::spawn`, `httparse` WebSocket preflight, and per-connection
    blocking state/credential setup.
- `crates/codex-router-proxy/src/websocket.rs`
  - release WebSocket path uses blocking `tungstenite`, cloned `TcpStream`
    revocation entries, and turn-gated upstream forwarding.
- `crates/codex-router-proxy/src/upstream.rs`
  - HTTP upstream still uses raw blocking `TcpStream` for HTTP and
    `reqwest::blocking` for HTTPS.
- `scripts/proof-matrix.sh`
  - missing at implementation entry; row command scaffold must be created
    before row receipts can become green.

## Execution Mode

Use controller-owned serial execution for T1 because it establishes the shared
runtime seam that later slices depend on. Use subagents only for bounded
read-only API/codebase questions or disjoint later implementation slices.

## First Slice: T1

Scope:

- introduce async runtime substrate and typed cutover seam
- define shared Hyper switchpoint types/interfaces
- add initial proof-matrix command scaffold
- do not claim release `serve` is async-complete
- do not implement HTTP/SSE body proxying or WebSocket pumps in T1

Allowed likely writes:

- `Cargo.toml`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-proxy/Cargo.toml`
- `crates/codex-router-proxy/src/server.rs`
- possible new proxy runtime modules under `crates/codex-router-proxy/src/`
- `scripts/proof-matrix.sh`
- tests required for T1 proof

T1 proof gates:

- failing test first for runtime config/switchpoint behavior
- failing proof-matrix script contract first
- targeted cargo tests for T1
- `cargo fmt --all -- --check`
- `scripts/proof-matrix.sh` rows introduced by the slice

## Stop Rules

Stop and return to planning if T1 reveals:

- the accepted plan requires an incomplete release-selected serve cutover
- Hyper/tokio-tungstenite API shape contradicts the spec handoff
- proof rows cannot be made executable inside the slice
- implementation would broaden into OAuth/keychain/session-picker/quota
  algorithm work
