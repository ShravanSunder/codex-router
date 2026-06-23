# Validation Proof Lane

Status: answered
Candidate evidence label: `candidate-validation-proof-r20-2026-06-23`
Security context: applicable

## Evidence Inspected

- Spec R1-R10, route inventory, WebSocket/security contract, and proof
  expectations.
- R20 ready ledger.
- Existing test anchors in selection, state, proxy, CLI, test-support, smoke
  scripts, and CI workflow.

## Accepted Proof Matrix Rows

| id | requirement / claim | owning task | proof layer | evidence source | red/green |
| --- | --- | --- | --- | --- | --- |
| VP-01 | Burn-down math covers pressure, surplus, salvage, reserve, blocked, stale, unknown, deterministic ordering | T1 | unit | `codex-router-selection::burn_down` tests | yes |
| VP-02 | RouteBand policy registry and unsupported-route-band flat envelope cannot drift | T1/T3 | unit/integration | selection + proxy classifier tests | yes |
| VP-03 | SQLite refresh read model preserves last-known windows, overlays stale by `now_unix_seconds`, and exposes redacted refresh status | T2 | integration | `codex-router-state` SQLite tests | yes |
| VP-04 | Runtime selector consumes persisted 5h+weekly assessment and partitions fairness/holds by route band | T3 | integration | proxy + state fixtures | yes |
| VP-05 | CLI status shares assessment semantics and does not reimplement limiting-window/reason math | T4 | unit/smoke | CLI renderer tests and quota status smoke | yes |
| VP-06 | Generated profile auth uses `env_key=CODEX_ROUTER_TOKEN`; installed Codex reaches router as `Authorization: Bearer` for HTTP/SSE and WebSocket | T5/T9/T10 | integration/e2e | profile tests, proxy auth, installed-Codex transcript | yes |
| VP-07 | HTTP/SSE auth-smuggling rejects only top-level forbidden fields and preserves nested prompt/tool/body values without emission | T5 | integration/security | proxy HTTP/SSE tests | yes |
| VP-08 | WebSocket preselection reads only allowlisted first-frame view and proves zero side effects on failures | T6 | integration/security | WebSocket call-order tests | yes |
| VP-09 | WebSocket selected account is pinned for connection lifetime and previous-response affinity occurs before weighted fallback | T6 | integration | proxy WebSocket + state tests | yes |
| VP-10 | Startup, first request, WebSocket first valid route, and status do not block on refresh | T7 | black-box | served router + delayed/failing fake refresh | yes |
| VP-11 | Every routed API has route-native success/fail-closed proof | T8 | route-native black-box | test-support route-native suite | yes |
| VP-12 | Installed Codex HTTP/SSE e2e proves generated profile, reset-aware choice, status agreement, and redacted transcript | T9 | installed-Codex e2e | `tests/smoke/installed_codex_mock.sh` | yes |
| VP-13 | Installed Codex WebSocket e2e proves WebSocket transport, selected-account pinning, and redacted first-frame transcript | T10 | installed-Codex e2e | installed-Codex mock harness | yes |
| VP-14 | Redaction proof covers labels, ids, tokens, auth headers, prompts, tool args, affinity keys/secrets, raw frames/bodies | T11 | unit/integration/smoke/security | captured artifacts and canary assertions | yes |

## Accepted Current Harness Gaps

- No `quota_refresh_status` table or `selector_inputs_for_route_band(..., now)`.
- `burn_down.rs` still exposes caller-supplied policy and lacks the flat R20
  route result envelope.
- Installed-Codex transcript still emits forbidden `first_frame_model`,
  `first_frame_has_input`, and `first_frame_stream`.
- Installed smoke accepts any routable upstream token instead of exact
  reset-aware chosen account.
- Current quota status smoke expects Unicode bars in plain output; spec requires
  ASCII plain proof.
- Route-native black-box coverage for every routed API is not complete.

## Validation Commands To Carry Into The Plan

Unit and integration:

- `cargo test -p codex-router-selection -- --list`
- `cargo nextest run -p codex-router-selection burn_down`
- `cargo nextest run -p codex-router-core redaction`
- `cargo nextest run -p codex-router-state selector refresh quota_refresh_status affinity`
- `cargo nextest run -p codex-router-proxy route_classifier repository_backed_selector authenticated_http_proxy authenticated_websocket websocket_first_frame blocking_websocket`
- `cargo nextest run -p codex-router-cli quota_status profile`

Smoke/e2e:

- `tests/smoke/quota_status_fixture.sh`
- `cargo test -p codex-router-test-support route_native_ -- --ignored --nocapture`
- `tests/smoke/installed_codex_mock.sh`

Full gate:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace`
- `cargo deny check`
- `cargo audit`

## Receipt

Answered by validation-proof lane. Parent accepted these rows as the proof
matrix seed, with exact commands subject to implementation-time test naming.
