# codex-router Spec Review

Date: 2026-06-20
Reviewed spec: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
Verdict after edits: ready for goal orchestration and plan creation, with open product choices carried into planning.

## Coverage

- Main spec: 383 lines before review.
- Evidence note: 81 lines before review.
- README: 15 lines before review.
- Parent read full spec in chunks `1-220` and `220-383`.
- Current Codex checkout: `d667082322`.
- Current Prodex checkout: `682e442a`.
- Official Codex manual helper reported the local manual cache current.
- DeepWiki `openai/codex` question confirmed the custom-provider surface at a repository level.

## Lanes

- `contract-and-scope`: needs revision.
- `architecture-boundaries`: needs revision.
- `security-threat-model`: needs revision.
- `validation-and-testability`: needs revision.
- `codex-routing-crux`: needs revision.

## Accepted Findings

1. `x-codex-turn-state` needed a precise envelope contract so router affinity does not corrupt upstream sticky-routing state.
2. WebSocket routing could not rely on handshake metadata alone because current Codex carries `previous_response_id` in `response.create` frames.
3. Local bearer-token delivery through `env_http_headers` needed a lifecycle: generation, delivery, missing-token behavior, rotation, and WebSocket revocation.
4. Unsupported provider paths needed fail-closed behavior before account selection.
5. Realtime/WebRTC routes needed explicit v1 exclusion.
6. `/v1/models` uses standard `ETag`; `x-models-etag` is WebSocket handshake behavior.
7. `/responses/compact` needed a compatibility note because current Codex may not send remote compact for the proposed custom provider name.
8. Reserved provider ids needed to include `amazon-bedrock`.
9. Latest Prodex `provider-core` and gateway surfaces needed explicit exclusion.
10. Rust quality gates and proof expectations needed stronger `must` language and a plan-create proof matrix seed.

## Edits Applied

- Added fail-closed handling for unsupported routes.
- Added Realtime/WebRTC out-of-scope language.
- Replaced vague turn-state language with a signed router envelope contract.
- Changed WebSocket state machine to route after the first local `response.create` frame.
- Added local auth and audit threat model.
- Added token rotation and old-WebSocket close behavior.
- Added audit event allowlist.
- Strengthened Rust quality standards from `should` to `must`.
- Added plan-create requirements/proof matrix seed.
- Added source-mining exclusions for Prodex `provider-core` and gateway surfaces.
- Clarified `/models` `ETag`, WebSocket `x-models-etag`, and compact compatibility.

## Contested Choices Carried Forward

- WebSocket in v1 remains a product choice. The edited spec keeps WebSocket in v1 but requires first-frame routing and proof.
- Secret backend selection remains open: macOS Keychain, 1Password-backed adapter, or hardened file store.
- `env_http_headers` remains the default local-auth carrier, with command-backed auth left as an open future design choice.
- Live multi-account OAuth/quota proof requires explicit approval language before execution.

## Proof Expectations Status

Proof expectations are present in the spec and now seed a later requirements/proof matrix. Plan creation must not start implementation until it expands these rows into exact commands, fixtures, expected observations, proof owners, and stale-proof guards.

## Next Step

Use `shravan-dev-workflow:orchestrator-goal` to create a durable goal contract whose next workflow is `shravan-dev-workflow:plan-create`.
