# Implementation Review Resolution: Account Onboarding And Quota UX

Date: 2026-06-21

## Review Scope

Reviewed the router-owned account import, quota refresh/status rendering, serve
background refresh worker, live quota adapter, secret/state path boundaries, and
docs/runbook contract.

Review agents:

- Noether: account/quota safety and endpoint boundary
- Poincare: state/persistence failure behavior
- Lagrange: CLI UX/docs contract
- Schrodinger: serve/background refresh and runtime clock behavior

## Accepted Findings And Fixes

- Quota OAuth tokens could be sent to arbitrary `--base-url` values.
  Fixed by allowing only `https://chatgpt.com/backend-api` by default and adding
  explicit test-only `--allow-insecure-quota-base-url` for loopback mocks.
- Provider or credential failures could leave stale positive selector snapshots.
  Fixed by replacing route quota state with failed zero-headroom snapshots/rows.
- Missing usable provider windows and past-reset/malformed windows could look
  successful.
  Fixed by failing closed, persisting failed status rows, and replacing selector
  snapshots with zero headroom.
- `.prodex` paths were not rejected.
  Fixed in both SQLite state and file secret store path guards.
- Read-only account/status commands could create SQLite state.
  Fixed `quota status` and `account list` to open existing SQLite read-only.
- `quota status` had no plain output contract and no explicit JSON rejection.
  Added `--format table|plain`; `--format json` is rejected until a schema
  exists.
- Enabled imported accounts without quota rows disappeared from status.
  Added synthesized `unknown` rows.
- `live quota` had a separate table renderer.
  Adapted live results into the shared quota status renderer.
- Serve used a fixed startup quota clock while background refresh used wall
  clock.
  Fixed serve to use a dynamic routing clock by default, with `--now-unix-seconds`
  as an explicit fixed-clock test seam shared by background refresh.

## Regression Proof

- `cargo test -p codex-router-cli -p codex-router-state -p codex-router-secret-store -p codex-router-auth`: passed after fixes.
- Final full gates are recorded in `implementation-execute-plan-brief.md`.

## Live Gate

Live OAuth/quota proof remains not run for this changed revision. Approval is
required before running `quota refresh` or `live quota` against real accounts.
