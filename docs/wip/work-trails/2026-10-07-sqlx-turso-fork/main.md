# 0.b: sqlx-turso fork brought into the monorepo — Lead trace

**Status: transferred (2026-10-07).** This file no longer receives updates. Its state was posted
to the board-design work thread (root `01a0f466-4201-7321-822e-018056ef60d0`) as message
`01a11695-7ba4-78b2-807c-6ea9d5af3f54`; 0.b continues there.

Before transfer: unshared. No board home: the work ran as a subagent of the board-design Lead.
Destination context: the board-design Lead's coordination (spec 2, project storage and
replication, depends on this driver).

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

### 2026-10-07 — Break found: Sync's TLS provider conflicts with Router's in unified builds

- **Assumed (mine, not owner-authorized):** adding the Turso crates as workspace members does
  not change how other crates build.
- **Found:** turso 0.8.1 depends on `hyper-rustls` with default features, which turn on rustls's
  `aws-lc-rs` provider. Router's crates turn on `ring`. In any build that unifies both (every
  `--workspace` build once the facade enables `sync` by default), rustls has two providers and
  `ClientConfig::builder()` panics: "Could not automatically determine the process-level
  CryptoProvider". Reproduced: `cargo test -p codex-router-proxy -p sqlx-turso --lib --
  claude_edge::upstream_endpoint` → 3 tests FAILED with that panic. The unified graph also breaks
  type inference in `collaboration-service` (`wakeup_projection.rs`, E0282/E0283: aws-lc-rs adds
  `From<()>` impls). Turso's own Sync worker builds its connector the same way
  (`with_native_roots()`), so it would panic too in such a build.
- **Meaning:** the conflict is real only where Sync and Router's TLS clients share a binary. No
  Router binary links the driver yet; spec 2 will. Making `sync` a default feature pushed the
  conflict into every workspace build for no consumer.
- **Disposition (reversible, in scope):** the facade's default features drop `sync`; Sync tests
  and Sync clippy run in their own `-p sqlx-turso --all-features` invocations, where rustls has
  only `aws-lc-rs`; the Sync test harness installs the `aws-lc-rs` process default explicitly so
  it also works in a mixed build. No Router crate changes. This reverses the all-five-defaults
  choice the Advisor agreed with, on this evidence.
- **Open question for the board-design Lead (spec 2, needs an owner decision):** when Router
  links Sync, which provider wins? Options: (a) Router's binaries install `ring` as the process
  default at startup and Router's TLS sites stop relying on implicit selection; (b) patch Turso
  to take `hyper-rustls` without default features, so only `ring` exists (a maintained Turso
  patch); (c) move Router to `aws-lc-rs` (changes Router's TLS crypto provider). Recommendation:
  (a), plus an upstream Turso issue for (b).

### 2026-10-07 — Implementation findings and repairs (all reversible, in scope)

- **Lockfile drift.** Turso 0.8.2 now exists; a fresh resolve floated eight internal Turso
  crates to 0.8.2 and bumped existing ICU/cc packages. Pinned all nine Turso packages to 0.8.1
  and restored every pre-existing package version (only `itertools` 0.14→0.12, inside
  `prost-derive`'s allowed range, for one proc-macro). Pinning new packages to the fork's lock
  then pulled `crossbeam-epoch` back to 0.9.18 (RUSTSEC-2026-0204); lifted to 0.9.21.
  `cargo deny check`: advisories, bans, licenses, sources ok.
- **Synced stores expose Turso internal tables** (`turso_cdc`, `turso_sync_last_change_id`,
  `__turso_internal_seq_…`) in `sqlite_schema`; the schema fingerprint excludes them. Spec-2
  schema validation must too.
- **Proc-macro test harness** cannot load `libstd` under the local signing runner; the macros
  crate has no unit tests, so its empty harness is off (`test = false`).
- **Path dependencies need versions** for `cargo deny` (wildcards = deny); added, as the other
  workspace path dependencies do.
- **CI split:** the three driver crates are linted and tested once with `--all-features` (Sync
  included, against the pinned `tursodb`), excluded from the workspace test run.

Decision without asking: kept the `itertools` unification rather than forcing two versions; it
is inside `prost-derive`'s declared range and compile-time only.

### 2026-10-07 — Workspace test failures traced to stale worktree build state, not the change

- First full run (as CI, Turso crates excluded): 3,278 of 3,282 passed; 4 timeout-themed tests
  failed (codex-router-auth ×2, codex-router-cli ×1, codex-router-proxy ×1). A second run:
  3 failed, a different set. All passed in isolation.
- `codex-router-auth` failed 3/3 at crate scope on this worktree while `main`, built as a
  plain source copy in a scratch target, passed 3/3 under the same machine load (load average
  25–56). Bypassing the signing test runner did not change that.
- The branch's committed tree, built as a plain copy in its own target, passed 3/3; the
  worktree's own sources with a fresh target directory passed 2/2. The resolved graph and
  features of `codex-router-auth`, `-proxy`, `-host` and `-cli` match `main` exactly (`cargo
  tree`, except `itertools` inside `prost-derive` for host and cli).
- Conclusion: the failures come from artifacts already in this worktree's `target/` (4.2 GB
  existed before this work started), not from the change. The proof run uses a fresh target
  directory.

### 2026-10-07 — Checkpoint: implementation and proof complete; design review pending

Commits (local only, not pushed): design `b5487aa7`; verbatim import `9eeff879`; trim/split/
wire `887b2395`; Router-path tests, native metadata and tooling `247138d9`; dependency policy
`e433cb9b`; CI and docs `20e54ffd`; this checkpoint. The design commit is SSH-signed; the last
two code commits are unsigned after two 1Password signing failures.

Proof on HEAD (fresh target directory unless noted), every command exit 0:

- `cargo fmt --all -- --check`; `python3 scripts/tooling/check-rust-file-size.py` (1,749 files).
- `cargo clippy --workspace --all-targets -- -D warnings` (0 diagnostics).
- `cargo clippy --locked -p sqlx-turso -p sqlx-turso-core -p sqlx-turso-macros --all-targets
  --all-features -- -D warnings` (0 diagnostics);
  `cargo check --locked -p sqlx-turso --lib --no-default-features --features runtime-tokio`.
- `python3 scripts/tooling/prepare-sqlx.py --check` (stock, worktree target) and
  `python3 scripts/tooling/prepare-sqlx-turso.py --check` (26 Turso descriptions).
- `cargo nextest run --profile ci --locked -p sqlx-turso -p sqlx-turso-core -p sqlx-turso-macros
  --all-features`: 82/82 passed (core 61; facade integration 21, Sync 5 against `tursodb`
  0.8.1); `cargo test --locked -p sqlx-turso --doc --all-features`: 1 passed.
- Workspace suite as CI runs it: 3,282/3,282 passed, 75 skipped; quota-reset harness 14/14.
- Tooling unit tests 33 OK; `cargo deny check` all four ok; `cargo audit` ok (one allowed
  warning that predates this branch).

Open for the board-design Lead (not blocking this port): rustls provider policy when Router
links Sync; inline blocking page IO in turso's statement step; the one production `expect` in
`to_url_lossy`; the CI download of the pinned `tursodb` release.

Unverified: CI itself (nothing pushed; Linux `tursodb` digests come from the release's own
`.sha256` files, not executed here; `aws-lc-sys` build on the Ubuntu runner); trybuild on a cold
CI cache; Sync over TLS (tests use loopback HTTP); nextest's intermittent "leaky" flag, attributed
to the signing runner, not proven.

Next: apply the independent design review's findings when they arrive.

### 2026-10-08 — Second independent review (Sol): F1–F3 confirmed and fixed; owner approvals

The first review round (Opus, F1–F16) is recorded in program design §12.3. This round's review
judged PR #137 not ready on three findings; each was checked against source and confirmed.

- **Owner approved:** the one production `expect` in `to_url_lossy`, and the CI download of
  the pinned `tursodb` release. Both leave the open list. Still open for spec 2: the rustls
  provider when Router links Sync, and the engine's inline blocking IO.
- **F2 (row lookahead):** confirmed; the stream returned the next row's error in place of the
  row it held. Lookahead removed; test fails before, passes after.
- **F1 (cached statements keep obsolete columns):** confirmed. The engine reprepares only on
  the first step, after the column list was taken. Fix: compare `PRAGMA schema_version` before
  reusing the cache; empty it on change. Add and rename tests failed before (`["id"]`; old
  name), pass after.
- **Break in my model, found while fixing F1:** I assumed the cookie check would see a Sync
  pull on the reader. It read the old cookie: the reader's cached `SELECT *`, abandoned by
  `fetch_one` after one row, was never reset (the engine resets on drop; the cache kept it
  alive), so the connection stayed on its old snapshot. Proven by a fully consumed warm-up
  passing. The same cause left `INSERT … RETURNING` read with `fetch_one` uncommitted
  (another connection counted 0 of 2 rows); it predates this round for multi-row RETURNING, and
  removing the lookahead would have extended it to single-row. Fix: the row stream resets its
  statement when it ends or is dropped, inside `Drop` so the SDK's operation guard is still held,
  skipped while panicking. Tests for another connection's `ADD COLUMN`, the pulled column and
  RETURNING failed before and pass after.
- **F3 (version prefix):** confirmed (`contains`); exact trimmed match. Test fails with
  `contains`, passes with the fix.

Decisions without asking: the pragma statement is held on the connection, outside the evictable
cache, so the check adds one step, not a parse, per persistent query; the running query is boxed
(clippy `large_enum_variant`).

Commits (signed): `a2e6bd8a` driver, `b16ad0bd` harness, `4da1b8be` design, this checkpoint.

Proof (worktree `target/`, every command exit 0): `cargo fmt --all -- --check`;
`check-rust-file-size.py` (1,758 files); clippy on the three crates, all targets, all features;
`cargo nextest run --profile ci` on the three crates with all features, 99/99 (core 71, facade
28 incl. Sync against `tursodb` 0.8.1); core without Sync, 61/61;
`prepare-sqlx-turso.py --check`.

Unverified: CI on the pushed head; the per-query cost of the schema-cookie step (not
measured); a reset failure in `Drop` is logged, not surfaced.
