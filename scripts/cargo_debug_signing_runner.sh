#!/usr/bin/env bash
# Cargo runner on Apple Silicon: signs Router's own executables with the debug
# code identity before running them, then runs every executable unchanged.
#
# Keychain approvals belong to a code identity. Linker-signed builds are
# identified by their hash, so each rebuild asks for Keychain access again.
# A Developer ID signature with a fixed identifier keeps one identity across
# rebuilds. The ".debug" identifiers keep debug builds a different identity
# from released builds, so a debug build never inherits approvals given to
# the production Router's Keychain item.
#
# Usage: as the cargo runner, `<executable> [arguments...]`; or
# `--sign-only <executable>...` to sign builds another process starts directly,
# such as the debug Router a debug-host example launches.
#
# Signing is a convenience: when it cannot happen the executable still runs,
# linker-signed, and Keychain access prompts again after the next rebuild.
set -euo pipefail

readonly DEVELOPER_ID_TEAM="974QD84WVC"

debug_identifier_for() {
  case "$(basename "$1")" in
    codex-router | agent-collaboration | agent-sessions)
      echo "dev.shravansunder.$(basename "$1").debug"
      ;;
  esac
}

warn_unsigned() {
  echo "cargo debug runner: $(basename "$1") stays linker-signed ($2); Keychain access prompts after each rebuild" >&2
}

has_debug_identity() {
  local signature
  signature="$(codesign -dv "$1" 2>&1)" || return 1
  grep -qx "Identifier=$2" <<<"${signature}" &&
    codesign --verify "$1" 2>/dev/null
}

sign_with_debug_identity() {
  local executable="$1"
  local debug_identifier="$2"
  local signing_identity signing_output

  if has_debug_identity "${executable}" "${debug_identifier}"; then
    return 0
  fi

  signing_identity="$(
    security find-identity -v -p codesigning |
      awk -v team="(${DEVELOPER_ID_TEAM})\"" 'index($0, "Developer ID Application") && index($0, team) { print $2; exit }'
  )"
  if [[ -z "${signing_identity}" ]]; then
    warn_unsigned "${executable}" "no Developer ID identity for team ${DEVELOPER_ID_TEAM}"
    return 0
  fi

  # Debug builds skip Apple's timestamp server so signing works offline.
  if signing_output="$(codesign --force --sign "${signing_identity}" --identifier "${debug_identifier}" --timestamp=none "${executable}" 2>&1)"; then
    return 0
  fi
  # Concurrent runs of one build race on codesign's temporary file; the loser
  # waits briefly for the winner's signature to land.
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if has_debug_identity "${executable}" "${debug_identifier}"; then
      return 0
    fi
    sleep 0.1
  done
  warn_unsigned "${executable}" "codesign failed: ${signing_output}"
}

if [[ "${1:-}" == "--sign-only" ]]; then
  shift
  for executable in "$@"; do
    debug_identifier="$(debug_identifier_for "${executable}")"
    if [[ -z "${debug_identifier}" ]]; then
      echo "cargo debug runner: $(basename "${executable}") is not a Router executable" >&2
      exit 1
    fi
    sign_with_debug_identity "${executable}" "${debug_identifier}"
  done
  exit 0
fi

executable="$1"
shift
debug_identifier="$(debug_identifier_for "${executable}")"
if [[ -n "${debug_identifier}" ]]; then
  sign_with_debug_identity "${executable}" "${debug_identifier}"
fi
exec "${executable}" "$@"
