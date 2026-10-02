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
set -euo pipefail

readonly DEVELOPER_ID_TEAM="974QD84WVC"

executable="$1"
shift

case "$(basename "${executable}")" in
  codex-router | agent-collaboration | agent-sessions)
    debug_identifier="dev.shravansunder.$(basename "${executable}").debug"
    ;;
  *)
    exec "${executable}" "$@"
    ;;
esac

signing_identity="$(
  security find-identity -v -p codesigning |
    awk -v team="(${DEVELOPER_ID_TEAM})\"" 'index($0, "Developer ID Application") && index($0, team) { print $2; exit }'
)"
if [[ -z "${signing_identity}" ]]; then
  echo "cargo debug runner: no Developer ID identity for team ${DEVELOPER_ID_TEAM}; running $(basename "${executable}") linker-signed, so Keychain access prompts after each rebuild" >&2
  exec "${executable}" "$@"
fi

if ! signing_output="$(codesign --force --sign "${signing_identity}" --identifier "${debug_identifier}" "${executable}" 2>&1)"; then
  echo "${signing_output}" >&2
  exit 1
fi
exec "${executable}" "$@"
