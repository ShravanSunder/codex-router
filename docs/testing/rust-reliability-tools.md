# Rust feedback and diagnostic tools

Use the repository's Rust toolchain. Bootstrap exact tool versions without
replacing globally installed tools:

```sh
scripts/tooling/bootstrap-tools.sh ci
export PATH="$PWD/tmp/rust-tools/bin:$PATH"
cargo nextest run -p communication-protocol
cargo nextest run --profile ci --workspace
```

The CI profile reports all failures, disables retries and reports tests exceeding
60 seconds. It does not terminate valid long-running integration tests; their
existing deadlines remain in force. Focused runs are feedback, not full workspace
proof. Doc tests run separately with `cargo test --workspace --doc`.

Versions live in `scripts/tooling/tool-versions.tsv`. Upgrade deliberately by
editing the declaration and rerunning bootstrap and affected checks. Bootstrap
validates binaries after cache restoration and installation. Audit data stays
current even when the executable version is pinned.

## Explicit diagnostics

```sh
scripts/tooling/bootstrap-tools.sh diagnostic
rustup component add llvm-tools-preview
scripts/tooling/rust-diagnostics.sh mutants codex-router-selection crates/codex-router-selection/src/eligibility.rs
scripts/tooling/rust-diagnostics.sh coverage codex-router-selection
```

Run diagnostics while other Cargo jobs are idle. Mutation testing is limited to
one selected source file, one worker, a 300-second build timeout and a 60-second
test-run timeout. It uses
scratch copies rather than modifying the checkout. A timeout or surviving mutant
is a finding to inspect, not proof that a test is missing; tool failures are not
successful reports. No blanket score gate is imposed.

Coverage has its own target directory, so tool cleanup does not discard ordinary
build output. Reports describe executed lines/regions under the selected package;
they do not prove assertion strength or concurrency correctness. Subprocesses
count only if separately instrumented and their profiles collected. These commands
do not claim branch or doc-test coverage. Cargo network offline mode differs from
SQLX_OFFLINE, which diagnostic builds explicitly preserve.

Keep any discovered defect grounded in an existing contract. Diagnostic tools do
not replace real database migration, process/socket/PTY or runtime acceptance tests.

## Checked account queries

The account query metadata is generated against an empty disposable database
created from the account crate's native migrations:

```sh
scripts/tooling/bootstrap-tools.sh sqlx
python3 scripts/tooling/prepare-sqlx.py
python3 scripts/tooling/prepare-sqlx.py --check
```

The wrapper supplies its own database URL and removes its temporary schema database
when finished. It never reads router or Codex home. Commit `.sqlx` changes with
query/schema changes. Metadata checks do not replace real migration tests.
Automation has a separate migration set; its first migration cutover does not
add checked automation queries or combine the databases.
