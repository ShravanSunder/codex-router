# sqlx-turso fork in the monorepo: requirements

Date: 2026-10-07. Status: draft for review. Specification: [spec.md](spec.md). Structure:
[program-design.md](program-design.md).

## Why

Router's project stores (board-design spec 2, project storage and replication) use the Turso
engine through SQLx checked queries and Turso's high-level Sync. The driver is a fork of
`avencera/sqlx-turso`, mirrored at `agent-collaboration/sqlx-turso` and already ported to Turso
0.8.1 in an investigation branch. This work makes that driver a first-class part of the
codex-router workspace: owned here, held to the workspace's lints and file rules, trimmed to
what Router uses, and proven by permanent tests. It is a library port. No Router store uses it
yet.

## Consumers

- Router project-store code (spec 2): opens one owned connection per store, runs migrations in an
  owned `BEGIN IMMEDIATE` transaction, writes with checked queries, pushes and pulls with Sync.
- Workspace CI and the developers who change these crates.

## Requirements

| ID | Requirement | Source |
|---|---|---|
| R1 | The driver lives as three workspace crates under `crates/`, keeping their names: `sqlx-turso` (facade), `sqlx-turso-core` (driver), `sqlx-turso-macros` (checked queries). `sqlx-turso-cli` is not imported. | Decided (brief) |
| R2 | Turso is pinned at exactly **0.8.1** and SQLx at exactly **0.9.0**; the lockfile resolves every Turso package to 0.8.1. | Decided (brief); trim-and-port §4 |
| R3 | The kept features are `runtime-tokio`, `macros`, `sync`, `migrate`, `chrono`. | Decided (brief) |
| R4 | The delete list is removed completely: `Any`, pool aliases and tests, FTS and index method, `time` and `uuid` codecs, shared-cache option, the experimental options matrix, `MigrateDatabase` create/drop lifecycle. Nothing deleted stays behind a flag. | Decided (brief); hard-cutover rule |
| R5 | Sync uses Turso's high-level Sync API. Its internal `turso-sync-io` thread is acceptable. No lower-SDK rewrite. | Decided (brief) |
| R6 | Every Rust file has at most 1,000 physical lines (aim about 600), split by responsibility, with names that say what they do. Upstream `options.rs` (1,389) and `value.rs` (1,046) are split. | Owner rule |
| R7 | The crates inherit workspace lints: no `unsafe`, no production `unwrap`, `expect`, `panic`, indexing or string slicing; `cargo clippy --workspace --all-targets -- -D warnings` passes. | Owner rule; `Cargo.toml` `[workspace.lints]` |
| R8 | No blocking IO that this code adds on the runtime; one Tokio runtime in our code. | Owner rule |
| R9 | Checked queries work against the native Turso engine, online and offline. Offline metadata for Turso is prepared from native migrations by repository tooling, checked in CI, and does not break the existing stock SQLx metadata check. | Brief §5; trim-and-port §3 |
| R10 | Schema constraints in fixtures and examples use PK, FK, NOT NULL and UNIQUE; CHECK only for booleans; no triggers; no libSQL; no sqld. | Owner rule; `AGENTS.md` |
| R11 | Proof: the driver's own tests pass on 0.8.1, and the Router-path probes become permanent integration tests — checked macros; Sync push and pull with a persistent reader; owned-transaction migrations with the FK-off rebuild policy; the known failures kept as negative tests. | Brief §4 |
| R12 | Upstream attribution and the license (MIT OR Apache-2.0, same as codex-router) are kept in a crate README. | Decided (brief) |
| R13 | The branch is public-safe: no local paths, session IDs or private links in committed files. | Brief boundaries |

## Success

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `python3 scripts/tooling/check-rust-file-size.py`, `python3 scripts/tooling/prepare-sqlx.py
  --check` and the new Turso metadata check all exit 0.
- `cargo test -p sqlx-turso-core -p sqlx-turso-macros -p sqlx-turso` passes, including the Sync
  integration tests against a real `tursodb` 0.8.1 sync server.
- The existing workspace test suite still passes.

## Out of scope

- Compile-time bind-arity checks (need the lower SDK) and read-only opens (deferred by the owner).
- Any Router store, gateway, lease or fencing code (spec 2).
- Production Sync hosting, WAN, TLS and authentication.
- A release or version bump: no binary changes behaviour.
</content>
</invoke>
