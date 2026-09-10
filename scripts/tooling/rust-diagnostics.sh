#!/usr/bin/env bash
# Explicit, bounded diagnostics; never an automatic score gate.
set -euo pipefail
repository_root="$(git rev-parse --show-toplevel)"
export PATH="$repository_root/tmp/rust-tools/bin:$PATH"
export SQLX_OFFLINE=true
mode="${1:-}"
package="${2:-}"
case "$package" in
  codex-router-selection|communication-protocol|codex-router-state|automation-storage) ;;
  *) echo "choose an affected package: codex-router-selection, communication-protocol, codex-router-state, automation-storage" >&2; exit 2 ;;
esac
"$repository_root/scripts/tooling/bootstrap-tools.sh" ci --check
"$repository_root/scripts/tooling/bootstrap-tools.sh" diagnostic --check
case "$mode" in
  coverage)
    [[ $# -eq 2 ]] || { echo "usage: $0 coverage PACKAGE" >&2; exit 2; }
    command -v cargo-llvm-cov >/dev/null || { echo "run scripts/tooling/bootstrap-tools.sh diagnostic first" >&2; exit 2; }
    # cargo-llvm-cov may clean its own artifacts; never share the ordinary target.
    export CARGO_TARGET_DIR="$repository_root/tmp/coverage-target"
    report_dir="$repository_root/tmp/rust-diagnostics/$package"
    mkdir -p "$report_dir"
    cargo llvm-cov nextest -p "$package" --lcov --output-path "$report_dir/coverage.lcov"
    echo "Coverage: $report_dir/coverage.lcov (instrumented test paths only; child/doc/branch coverage not assumed)"
    ;;
  mutants)
    [[ $# -eq 3 ]] || { echo "usage: $0 mutants PACKAGE SOURCE_FILE" >&2; exit 2; }
    case "$package" in codex-router-selection|communication-protocol) ;; *) echo "mutation scope is selection/protocol logic" >&2; exit 2 ;; esac
    source_file="$3"
    case "$source_file" in "crates/$package/src/"*.rs) ;; *) echo "source must belong to the selected package" >&2; exit 2 ;; esac
    [[ "$source_file" != *..* && -f "$source_file" ]] || { echo "invalid source file" >&2; exit 2; }
    command -v cargo-mutants >/dev/null || { echo "run scripts/tooling/bootstrap-tools.sh diagnostic first" >&2; exit 2; }
    export CARGO_TARGET_DIR="$repository_root/tmp/mutants-target"
    mkdir -p "$repository_root/tmp/rust-diagnostics/$package"
    cargo mutants -p "$package" --file "$source_file" --test-tool nextest --jobs 1 --build-timeout 300 --timeout 60 --output "$repository_root/tmp/rust-diagnostics/$package"
    ;;
  *) echo "usage: $0 coverage PACKAGE | mutants PACKAGE SOURCE_FILE" >&2; exit 2 ;;
esac
