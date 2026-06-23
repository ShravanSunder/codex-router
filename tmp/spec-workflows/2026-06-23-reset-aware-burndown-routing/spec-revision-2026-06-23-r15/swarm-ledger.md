# R15 Reset-Aware Burn-Down Routing Spec Revision Ledger

Date: 2026-06-23
Phase: spec-creation-swarm
Status: revised after R14 needs-revision findings

## Source Inputs

- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r14/review-ledger.md`
- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-proxy/src/local_auth.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- sibling installed-Codex source under `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/model-provider-info/src/lib.rs`

## Lanes

| Lane | Agent | Status | Accepted finding |
| --- | --- | --- | --- |
| auth-profile-compatibility | Turing | accepted | Generated profile must stay `env_key = "CODEX_ROUTER_TOKEN"` for installed Codex; local router must accept `Authorization: Bearer` for HTTP/SSE and WebSocket. |
| selection-envelope-cooldown | Avicenna | accepted | Unknown fallback, unsupported route bands, selected-pool reason, and runtime cooldown/pinning need one route-result contract. |
| status-refresh-transcript | Boyle | accepted | Refresh persistence needs repository-level success/failure operations; smoke transcripts must forbid derived first-frame fields such as `first_frame_model` and `first_frame_has_input`. |

## Parent Synthesis

R14 was correct that the spec contradicted the live installed-Codex path. R15
chooses the current proven path as the contract: generated profile `env_key`
creates local `Authorization: Bearer`; `X-Codex-Router-Token` remains a
manual/compatibility carrier. The spec now rejects only ambiguous or unsafe
carriers: query, cookie, body token, subprotocol token, and mismatched mixed
accepted carriers.

R15 also normalizes route assessment output. Supported and unsupported route
bands now share one route-level shape: `route_result`, `selected_pool`,
`selected_pool_reason`, `preferred_next_account_id`, `weighted_candidates`, and
`accounts`. Unknown quota is selectable only when all known usable/reserve pools
are empty, and runtime cooldown/affinity can reuse only accounts still present
in the current `weighted_candidates`.

WebSocket preselection now allows installed-Codex direct response-create frames
through bounded structural booleans while forbidding raw direct-payload values
from influencing selection or appearing in logs, traces, audit, or smoke proof.

## Reviewer Attack Points

- Verify the generated-profile auth contract is consistent with installed Codex
  and does not accidentally reopen query/cookie/body/subprotocol fallback.
- Verify WebSocket direct-payload structural checks are narrow enough and the
  raw field values remain forbidden in proof artifacts.
- Verify unknown fallback and cooldown/pinning cannot keep using a blocked,
  excluded, exhausted, or non-current-pool account.
- Verify status/JSON/runtime use the same route-result fields and that
  unsupported route bands cannot advance selector state.
- Verify refresh success/failure repository operations are proofable against
  SQLite without blocking startup or request routing.
