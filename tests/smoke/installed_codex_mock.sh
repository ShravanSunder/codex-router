#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

export PATH="${HOME}/.cargo/bin:${PATH}"

cd "${repo_root}"

if command -v cargo >/dev/null 2>&1 && cargo --version >/dev/null 2>&1; then
  cargo_command=(cargo)
elif command -v rustup >/dev/null 2>&1 && rustup run 1.95.0 cargo --version >/dev/null 2>&1; then
  cargo_command=(rustup run 1.95.0 cargo)
elif [[ -x "${HOME}/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo" ]]; then
  cargo_command=("${HOME}/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo")
else
  echo "cargo or rustup with toolchain 1.95.0 is required" >&2
  exit 127
fi

"${cargo_command[@]}" test \
  -p codex-router-test-support \
  installed_codex_ \
  -- \
  --ignored \
  --nocapture
