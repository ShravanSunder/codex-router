#!/usr/bin/env bash
set -euo pipefail
# Separate feature/link lanes keep libsql's C engine away from SQLx's SQLite.
manifest=spikes/turso-replication/Cargo.toml
proof_directory="tmp/practices-research/2026-10-04-turso-replication/final-proof"
mkdir -p "$proof_directory"
cargo build --locked --manifest-path "$manifest"
cargo build --locked --manifest-path "$manifest" --no-default-features --features embedded --bin embedded-replica-spike
spikes/turso-replication/target/debug/embedded-replica-spike | tee "$proof_directory/embedded.txt"
replica_path=$(python3 - "$proof_directory/embedded.txt" <<'PY'
import pathlib
import sys
for line in pathlib.Path(sys.argv[1]).read_text().splitlines():
    prefix = "SQLX_PROBE embedded file="
    if line.startswith(prefix):
        print(line.removeprefix(prefix))
        break
else:
    raise SystemExit("missing embedded replica proof path")
PY
)
spikes/turso-replication/target/debug/sqlx-probe "$replica_path" | tee "$proof_directory/sqlx-embedded.txt"
spikes/turso-replication/target/debug/turso-replication-spike all | tee "$proof_directory/sync-and-compatibility.txt"
cargo fmt --manifest-path "$manifest" -- --check
cargo clippy --locked --manifest-path "$manifest" --all-targets -- -D warnings
cargo clippy --locked --manifest-path "$manifest" --no-default-features --features embedded --bin embedded-replica-spike -- -D warnings
