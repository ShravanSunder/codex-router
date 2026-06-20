# codex-router Research Evidence

Date: 2026-06-20

Current source snapshots used by the spec review:

- Codex: `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex`, commit `d667082322`.
- Prodex: `/Users/shravansunder/Documents/dev/open-source/ai-dev/prodex`, commit `682e442a`.
- Archived old fork: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router-prodex-fork-archive-2026-06-20`.
- Official Codex manual cache: `/var/folders/4f/697ggy6x26q8kh9qb2js4xnc0000gn/T/openai-docs-cache/codex-manual.md`; helper reported current on 2026-06-20.

## Codex Evidence

- Codex profiles overlay `~/.codex/<profile>.config.toml` and use top-level config keys.
- Codex custom providers define `base_url`, `wire_api`, auth behavior, headers, retry/timeout fields, `requires_openai_auth`, and `supports_websockets`.
- Codex appends routes to provider `base_url`; a router exposing `/v1/responses` should use `base_url = ".../v1"`.
- Codex WebSocket support is gated by `supports_websockets`.
- Codex may send `x-codex-turn-state` and related metadata; router state must treat it as opaque, short-lived routing data.
- Codex uses custom provider traffic beyond plain `/responses`: `/models`, model etags, memories trace summarization, and possibly compact routes.
- Current Codex WebSocket routing metadata such as `previous_response_id` can appear in the first `response.create` frame rather than the upgrade handshake.
- Current Codex uses standard `ETag` for `/models`; `x-models-etag` is part of Responses WebSocket handshake behavior.
- Current Codex contains Realtime/WebRTC routes (`/v1/realtime/calls`, `/v1/realtime`), which the v1 spec explicitly excludes.
- Current Codex may not send remote `/responses/compact` for a custom provider named `Codex Router`, but the router keeps compatibility coverage for installed builds that do.

Local source references inspected:

- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/model-provider-info/src/lib.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/config/src/profile_toml.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/config/src/loader/mod.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/client.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/responses.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/responses_websocket.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/models.rs`
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/memories.rs`
- `/var/folders/4f/697ggy6x26q8kh9qb2js4xnc0000gn/T/openai-docs-cache/codex-manual.md`

## Prodex Source-Mining Evidence

Useful source-mining areas:

- OAuth/account parsing and refresh-needed logic
- quota usage models and quota-window tests
- refresh lease and single-flight coordination
- secret-store trait shape and hardened file-write ideas
- HTTP/SSE response forwarding, header filtering, and bounded stream observation
- WebSocket proxy mechanics

Areas to exclude:

- shared Codex filesystem management
- Codex home copy, symlink, migrate, or repair flows
- history/session merging
- Codex launcher argument/profile rewriting
- Claude, Gemini, Anthropic, and generic multi-provider runtime paths
- smart context and prompt rewriting
- overload/5xx retry policy
- health/circuit/provider-gate machinery
- current Prodex `provider-core` architecture
- current Prodex gateway admin, virtual-key, billing, metrics, guardrail, SCIM, SSO, tenant, Redis, Postgres, OpenAPI, and route-strategy surfaces

Archived source path:

- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router-prodex-fork-archive-2026-06-20`

## Rust Quality Evidence

Codex repo standards worth copying in smaller form:

- Rust 2024
- workspace dependencies and lints
- `rustfmt` item import granularity
- clippy denies for async lock hazards, production unwrap/expect, needless/manual/redundant patterns, and warning-free builds
- `cargo nextest`
- `cargo deny`
- `cargo audit`
- dependency hygiene checks

Codex repo standards not worth copying initially:

- Bazel
- large release matrices
- custom nextest shard/archive machinery
- Codex-specific workflow lint scripts
- heavyweight post-merge CI split

## Adversarial Findings Accepted Into Spec

- WebSocket routing is connection-scoped, not per-message.
- WebSocket account selection must wait for the first local `response.create` frame, because current Codex can place continuation metadata there rather than in the handshake.
- A transparent proxy still participates in protocol boundaries when it classifies precommit auth/quota failures.
- Localhost is not authentication.
- `env_http_headers` requires an operational token generation and delivery story.
- A clean greenfield repo is simpler than subtractive surgery from a 40-crate Prodex fork.
- The spec needs an explicit compatibility matrix because Codex traffic is more than generic OpenAI-compatible `/responses`.
