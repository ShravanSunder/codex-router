#!/usr/bin/env bash
set -euo pipefail
# Run from the repository root. Downloads stay inside this worktree.
mkdir -p tmp/tools
for release in 'turso v0.8.1 turso_cli-aarch64-apple-darwin' 'libsql libsql-server-v0.24.32 libsql-server-aarch64-apple-darwin'; do
  read -r repository tag archive_name <<< "$release"
  base_url="https://github.com/tursodatabase/${repository}/releases/download/${tag}"
  curl --fail --location --silent --show-error "${base_url}/${archive_name}.tar.xz" -o "tmp/tools/${archive_name}.tar.xz"
  curl --fail --location --silent --show-error "${base_url}/${archive_name}.tar.xz.sha256" -o "tmp/tools/${archive_name}.tar.xz.sha256"
  python3 - "tmp/tools/${archive_name}.tar.xz" "tmp/tools/${archive_name}.tar.xz.sha256" <<'PY'
import hashlib
import pathlib
import sys
archive = pathlib.Path(sys.argv[1])
expected = pathlib.Path(sys.argv[2]).read_text().split()[0]
actual = hashlib.sha256(archive.read_bytes()).hexdigest()
if actual != expected:
    raise SystemExit(f"release digest mismatch: {archive}")
print(f"release digest verified: {archive.name}")
PY
  tar -xJf "tmp/tools/${archive_name}.tar.xz" -C tmp/tools
 done
tmp/tools/turso_cli-aarch64-apple-darwin/tursodb --version
tmp/tools/libsql-server-aarch64-apple-darwin/sqld --version
