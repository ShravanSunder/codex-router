#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "sqlite3 is required" >&2
  exit 127
fi

mkdir -p "${repo_root}/tmp/smoke"
scratch_root="$(mktemp -d "${repo_root}/tmp/smoke/sync-debug-sqlite-destination-guard.XXXXXX")"
mkdir -p "${scratch_root}/fake-repo/scripts" "${scratch_root}/sources"

fake_repo="${scratch_root}/fake-repo"
cp "${repo_root}/scripts/sync-debug-sqlite.sh" "${fake_repo}/scripts/sync-debug-sqlite.sh"
chmod +x "${fake_repo}/scripts/sync-debug-sqlite.sh"

router_source="${scratch_root}/sources/router.sqlite"
codex_source="${scratch_root}/sources/codex.sqlite"
sqlite3 "${router_source}" "PRAGMA user_version = 1;"
sqlite3 "${codex_source}" "PRAGMA user_version = 1;"

assert_refuses_escape_without_creating_destination() {
  local label="$1"
  local router_destination="$2"
  local codex_destination="$3"
  local forbidden_router_path="$4"
  local forbidden_codex_path="$5"
  local output_file="${scratch_root}/${label}.stderr"

  set +e
  (
    cd "${fake_repo}"
    ROUTER_SOURCE="${router_source}" \
      CODEX_SOURCE="${codex_source}" \
      CODEX_ROUTER_DEBUG_ROUTER_ROOT="${router_destination}" \
      CODEX_ROUTER_DEBUG_CODEX_HOME="${codex_destination}" \
      bash scripts/sync-debug-sqlite.sh
  ) >"${output_file}.stdout" 2>"${output_file}"
  local status=$?
  set -e

  if [[ "${status}" -ne 2 ]]; then
    echo "${label}: expected exit 2, got ${status}" >&2
    cat "${output_file}" >&2
    exit 1
  fi

  if [[ -e "${forbidden_router_path}" || -e "${forbidden_codex_path}" ]]; then
    echo "${label}: escape destination was created before refusal" >&2
    find "${scratch_root}" -maxdepth 4 -print >&2
    exit 1
  fi

  if ! grep -q "refusing to write debug" "${output_file}"; then
    echo "${label}: refusal message missing" >&2
    cat "${output_file}" >&2
    exit 1
  fi
}

assert_refuses_escape_without_creating_destination \
  "dotdot" \
  "${fake_repo}/tmp/../outside-router" \
  "${fake_repo}/tmp/../outside-codex" \
  "${fake_repo}/outside-router" \
  "${fake_repo}/outside-codex"

mkdir -p "${fake_repo}/tmp" "${scratch_root}/outside-symlink-target"
ln -s "${scratch_root}/outside-symlink-target" "${fake_repo}/tmp/escape"

assert_refuses_escape_without_creating_destination \
  "symlink" \
  "${fake_repo}/tmp/escape/router" \
  "${fake_repo}/tmp/escape/codex" \
  "${scratch_root}/outside-symlink-target/router" \
  "${scratch_root}/outside-symlink-target/codex"

rm -rf "${fake_repo}/tmp"
mkdir -p "${scratch_root}/outside-tmp-root"
ln -s "${scratch_root}/outside-tmp-root" "${fake_repo}/tmp"

assert_refuses_escape_without_creating_destination \
  "tmp-root-symlink" \
  "${fake_repo}/tmp/dev-state/codex-router" \
  "${fake_repo}/tmp/dev-state/codex" \
  "${scratch_root}/outside-tmp-root/dev-state/codex-router" \
  "${scratch_root}/outside-tmp-root/dev-state/codex"

rm -rf "${fake_repo}/tmp"
mkdir -p \
  "${fake_repo}/tmp/dev-state/codex-router" \
  "${fake_repo}/tmp/dev-state/codex" \
  "${scratch_root}/outside-db-targets"
sqlite3 "${scratch_root}/outside-db-targets/router-target.sqlite" "PRAGMA user_version = 91;"
sqlite3 "${scratch_root}/outside-db-targets/codex-target.sqlite" "PRAGMA user_version = 92;"
ln -s "${scratch_root}/outside-db-targets/router-target.sqlite" \
  "${fake_repo}/tmp/dev-state/codex-router/state.sqlite"
ln -s "${scratch_root}/outside-db-targets/codex-target.sqlite" \
  "${fake_repo}/tmp/dev-state/codex/state_5.sqlite"

set +e
(
  cd "${fake_repo}"
  ROUTER_SOURCE="${router_source}" \
    CODEX_SOURCE="${codex_source}" \
    bash scripts/sync-debug-sqlite.sh
) >"${scratch_root}/db-file-symlink.stdout" 2>"${scratch_root}/db-file-symlink.stderr"
db_file_symlink_status=$?
set -e

if [[ "${db_file_symlink_status}" -ne 2 ]]; then
  echo "db-file-symlink: expected exit 2, got ${db_file_symlink_status}" >&2
  cat "${scratch_root}/db-file-symlink.stderr" >&2
  exit 1
fi

if ! grep -q "refusing to write debug" "${scratch_root}/db-file-symlink.stderr"; then
  echo "db-file-symlink: refusal message missing" >&2
  cat "${scratch_root}/db-file-symlink.stderr" >&2
  exit 1
fi

router_target_version="$(
  sqlite3 "${scratch_root}/outside-db-targets/router-target.sqlite" "PRAGMA user_version;"
)"
codex_target_version="$(
  sqlite3 "${scratch_root}/outside-db-targets/codex-target.sqlite" "PRAGMA user_version;"
)"
if [[ "${router_target_version}" != "91" || "${codex_target_version}" != "92" ]]; then
  echo "db-file-symlink: destination symlink target was modified" >&2
  exit 1
fi

echo "sync debug sqlite destination guard smoke ok: ${scratch_root}"
