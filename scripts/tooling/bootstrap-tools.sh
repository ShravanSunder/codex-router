#!/usr/bin/env bash
# Resolve exact repository tools without replacing globally installed binaries.
set -euo pipefail
repository_root="$(git rev-parse --show-toplevel)"
versions_file="${ROUTER_TOOL_VERSIONS_FILE:-$repository_root/scripts/tooling/tool-versions.tsv}"
install_root="${ROUTER_TOOL_INSTALL_ROOT:-$repository_root/tmp/rust-tools}"
selected_role="${1:-ci}"
check_only="${2:-}"
[[ $# -le 2 && ( -z "$check_only" || "$check_only" == --check ) ]] || { echo "usage: $0 [ci|sqlx|diagnostic|all] [--check]" >&2; exit 2; }
case "$selected_role" in ci|sqlx|diagnostic|all) ;; *) echo "usage: $0 [ci|sqlx|diagnostic|all]" >&2; exit 2 ;; esac

installed_version() {
  local executable="$1" output subcommand
  subcommand="${executable##*/}"
  subcommand="${subcommand#cargo-}"
  [[ -x "$executable" ]] || return 1
  output="$("$executable" "$subcommand" --version)" || return 1
  # nextest prints extra release metadata; compare its first-line version only.
  awk 'NR == 1 {print $2; exit}' <<< "$output"
}

selected_count=0
while read -r crate version binary role extra; do
  [[ -n "${crate:-}" && "$crate" != \#* ]] || continue
  if [[ -n "${extra:-}" || ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ || ! "$crate" =~ ^[a-z0-9-]+$ || ! "$binary" =~ ^[a-z0-9-]+$ ]]; then
    echo "invalid tool declaration" >&2; exit 2
  fi
  [[ "$selected_role" == all || "$selected_role" == "$role" ]] || continue
  selected_count=$((selected_count + 1))
  executable="$install_root/bin/$binary"
  actual="$(installed_version "$executable" || true)"
  if [[ "$actual" != "$version" ]]; then
    if [[ "$check_only" == --check ]]; then
      echo "missing or mismatched $binary; run scripts/tooling/bootstrap-tools.sh $selected_role" >&2; exit 2
    fi
    install_arguments=(install "$crate" --version "=$version" --locked --root "$install_root" --force)
    if [[ "$crate" == sqlx-cli ]]; then
      install_arguments+=(--no-default-features --features "sqlite,rustls")
    fi
    cargo "${install_arguments[@]}"
  fi
  actual="$(installed_version "$executable" || true)"
  if [[ "$actual" != "$version" ]]; then
    echo "tool version verification failed: $binary expected $version" >&2; exit 1
  fi
  echo "$binary $actual"
done < "$versions_file"

if [[ "$selected_count" -eq 0 ]]; then
  echo "no tools declared for role $selected_role" >&2; exit 2
fi
