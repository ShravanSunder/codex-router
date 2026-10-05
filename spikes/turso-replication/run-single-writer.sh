#!/usr/bin/env bash
set -euo pipefail
# From repository root. This does not run any two-offline-writer case.
manifest=spikes/turso-replication/Cargo.toml
proof=tmp/practices-research/2026-10-05-turso-single-writer/reproduced
mkdir -p "$proof"
cargo build --locked --manifest-path "$manifest" --features single-writer --bin single-writer-spike --bin offline-writer
spikes/turso-replication/target/debug/single-writer-spike all | tee "$proof/single-writer.txt"
# Checked-query metadata is committed under the standalone project's .sqlx/.
SQLX_OFFLINE=true cargo build --locked --manifest-path "$manifest" --no-default-features --features driver --bin driver-probe
spikes/turso-replication/target/debug/driver-probe | tee "$proof/sqlx-turso.txt"
SQLX_OFFLINE=true cargo check --locked --manifest-path "$manifest" --no-default-features --features offline-proof --bin driver-probe
cargo build --locked --manifest-path "$manifest" --no-default-features --features runtime-probe --bin runtime-probe
spikes/turso-replication/target/debug/runtime-probe | tee "$proof/runtime.txt"
# Real compile-time rejection; this binary permanently encodes invalid SQL.
SQLX_OFFLINE=true cargo build --locked --manifest-path "$manifest" --no-default-features --features driver --bin driver-seed
spikes/turso-replication/target/debug/driver-seed tmp/turso-driver/metadata.db
if SQLX_OFFLINE=false DATABASE_URL="turso://$PWD/tmp/turso-driver/metadata.db" \
    cargo check --locked --manifest-path "$manifest" --no-default-features --features negative-query --bin driver-negative-query > "$proof/negative-query.txt" 2>&1; then
  echo 'ERROR: invalid column unexpectedly passed compile checking' >&2
  exit 1
fi
rg --fixed-strings 'no such column: missing_column' "$proof/negative-query.txt"
cargo fmt --manifest-path "$manifest" -- --check
cargo clippy --locked --manifest-path "$manifest" --features single-writer --bin single-writer-spike --bin offline-writer -- -D warnings
SQLX_OFFLINE=true cargo clippy --locked --manifest-path "$manifest" --no-default-features --features driver --bin driver-probe --bin driver-seed -- -D warnings
cargo clippy --locked --manifest-path "$manifest" --no-default-features --features runtime-probe --bin runtime-probe -- -D warnings
