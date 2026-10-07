# 0.b: sqlx-turso fork brought into the monorepo — Lead trace

**Status: unshared.** No board home: the work runs as a subagent of the board-design Lead, which
cannot receive Router messages for this run. Destination context: the board-design Lead's
coordination (spec 2, project storage and replication, depends on this driver).

- Goal: the Turso SQLx driver lives as workspace crates (`sqlx-turso`, `sqlx-turso-core`,
  `sqlx-turso-macros`), ported to Turso 0.8.1, trimmed to what Router uses, split to the
  file-size rule, tested.
- Worktree: this repository checkout, branch `feat/sqlx-turso-fork`, from main `77a7f94f`.
- Source: the `agent-collaboration/sqlx-turso` mirror of `avencera/sqlx-turso`, upstream
  `b7e5fab9`, 0.8.1 port commit `1833f652` (local branch `investigate/turso-0.8.1`).
- Lead: Claude Opus 5.5 subagent of the board-design Lead. Advisor: GPT 6.1 Sol xhigh.
- Boundaries: no push (public repo), no merge/tags/release, no production Router/Host, no
  libSQL/sqld/triggers, CHECK only for booleans, one Tokio runtime, no production panics.

## Entries (newest last)

### 2026-10-07 — Orientation

Read: the brief; the three investigation reports (trim-and-port, Router-path proof,
many-databases); repo `AGENTS.md`; the Rust rubric; `check-rust-file-size.py`;
`prepare-sqlx.py`; `ci.yml`; workspace `Cargo.toml`; every fork source file; sqlx 0.9.0
macro/CLI sources for offline lookup; turso 0.8.1 statement stepping.

Facts that shape the design (evidence, not claims):

- sqlx 0.9 macros resolve offline data from `SQLX_OFFLINE_DIR`, then
  `<invoking crate>/.sqlx`, then `<workspace>/.sqlx`
  (`sqlx-macros-core-0.9.0/src/query/mod.rs:97-101`). A crate-local `.sqlx` isolates Turso
  metadata from the stock cache.
- The stock `prepare-sqlx.py` write mode replaces every `query-*.json` in the root `.sqlx`
  (`replace_query_metadata`), so Turso metadata cannot live there. Its `cargo sqlx prepare`
  runs `cargo check --package <four stock packages>`, so it never compiles the Turso crates.
- The Turso sync server lives only in the unpublished CLI (`cli/sync_server.rs`); there is no
  library server. Sync integration tests need the `tursodb` 0.8.1 release binary. Release
  assets carry per-target `.sha256` files.
- turso 0.8.1 `Statement::step` runs pending page IO inline (`stmt.run_io()`) inside `poll`
  (`turso-0.8.1/src/lib.rs:414-445`): short blocking file IO on the calling runtime thread.
  The decided high-level API keeps this; it is a named tradeoff, not something this port fixes.
- Workspace `sqlx` enables `sqlite`; inheriting it would link the SQLite C engine into the
  Turso crates' own builds and test binaries.
- Workspace lints forbid `unsafe_code`; the fork's UI test uses `unsafe { set_var }`.
- Upstream has no LICENSE file; license is declared in manifests (MIT OR Apache-2.0).

### 2026-10-07 — Decisions made without asking (reversible, in scope)

Recorded in `docs/specs/2026-10-07-sqlx-turso-fork/program-design.md` with reasons. Summary:

1. Facade default features = the five kept features, so `--workspace --all-targets` lint and
   test cover every kept path; a `--no-default-features --features runtime-tokio` check keeps
   the gated branches compiling.
2. Extra trims beyond the decided delete list, all "not used by Router": `json`, VFS name,
   custom IO, immutable, the read-only option (explicitly rejected instead), the generic PRAGMA
   list (typed `foreign_keys` instead), Vacuum and the rest of the experimental matrix, URL sync
   parameters, empty marker traits, upstream examples.
3. Workspace `sqlx` entry becomes engine-agnostic and exact (`=0.9.0`, `runtime-tokio`);
   existing members add `features = ["sqlite"]` explicitly.
4. Turso metadata lives in `crates/sqlx-turso/.sqlx/`, prepared by a new
   `scripts/tooling/prepare-sqlx-turso.py`; the stock script and cache are untouched.
5. Sync integration tests require the pinned `tursodb` 0.8.1 binary, installed by a new
   checksum-pinned installer into `tmp/rust-tools/bin/`; missing binary fails loudly.
</content>
</invoke>

### 2026-10-07 — Advisor critique applied; design committed

The Advisor (GPT 6.1 Sol xhigh, read-only) agreed with the workspace `sqlx` change, the
crate-local metadata, the pinned `tursodb` installer and all-five defaults, and changed: the
SQLite-free guarantee is scoped to separate Turso builds and gets a graph test; the test plan
regains D1's transactional snapshots with exact agreement and interleaving, plus a replicated
FK-off rebuild; S4 separates tested limitations from source-established ones; Vacuum and
read-only wording corrected; harness gets bounded shutdown and an override version check.
Rejected one point (chrono parse error as source) with reason. Full table:
`docs/specs/2026-10-07-sqlx-turso-fork/program-design.md` §11.

Open question for the board-design Lead (not blocking this port): turso 0.8.1 runs synchronous
`pread`/`pwrite`/`fsync` inside `Statement::step`'s `poll`, on the calling runtime worker. The
owner accepted the Sync thread, not this. Spec 2 must choose: accept the exception (measure it),
or isolate SQL execution from executor workers.

Decision without asking: one production `#[expect(clippy::expect_used)]` on parsing the
constant `turso:` URL in `to_url_lossy`, because the trait cannot return an error and SQLx's
default is `unimplemented!()`.
