# codex-router Greenfield Product Spec

Date: 2026-06-20
Status: draft spec

## Product Intent

`codex-router` is a small local proxy for Codex CLI traffic. Its job is to let one Codex profile talk to a local router, while the router chooses among multiple upstream OpenAI/ChatGPT OAuth accounts based on quota state and routing correctness.

The router must be fast to start, must stay out of Codex's way, and must not become a Codex launcher, installer, home manager, session repair tool, prompt rewriter, context optimizer, retry engine, or multi-provider gateway.

The router exists because account choice and quota balancing are account concerns. Codex should continue deciding how to run a turn, which transport to use, which protocol fields to send, how to stream, how to compact, and how to manage sessions.

## Product Laws

1. Codex owns Codex behavior.
2. The router owns account credentials, quota snapshots, route selection, and upstream auth injection.
3. The router must not read, scan, repair, migrate, symlink, or rewrite Codex home state at runtime.
4. The router must not launch Codex or rewrite Codex command-line arguments.
5. The router must preserve Codex requests and responses except for local auth removal, upstream auth injection, hop-by-hop header handling, and explicitly specified routing metadata.
6. Rotation is allowed only before an upstream response is committed, and only for explicit account, auth, or quota reasons.
7. Codex transport failures belong to Codex or the network. The router must not create its own retry, timeout, overload, health, or circuit-breaker policy layer.
8. WebSocket routing is connection-scoped in v1. HTTP/SSE routing is request-scoped.

## Activation Model

Codex activates the router through a Codex profile file, not through a wrapper command.

The intended Codex profile is a user-level file such as:

```toml
model_provider = "codex-router"

[model_providers.codex-router]
name = "Codex Router"
base_url = "http://127.0.0.1:18733/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true
env_http_headers = { "X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN" }
```

The router may provide a command that prints this profile text. It must not silently apply it to `~/.codex`. Any command that writes Codex configuration is a home-level mutation and requires a preview-first workflow.

The profile must use a custom provider id. It must not rely on `openai_base_url`, and it must not use built-in provider ids: `openai`, `amazon-bedrock`, `ollama`, or `lmstudio`. Codex allows only limited AWS field overrides for the built-in `amazon-bedrock` provider; `codex-router` is not that provider.

`requires_openai_auth = false` is required because Codex must not attach its own OpenAI credential to router-bound requests. The router attaches upstream account auth after local router authentication and route selection.

The router does not require separate Codex session stores, separate Codex histories, separate Codex logs, or separate Codex installs. A Codex session is still a Codex session.

## Supported Codex Traffic

The router is a Codex model-provider proxy. It must support the Codex custom-provider traffic surface, not merely generic OpenAI-compatible traffic.

Required routes:

- `POST /v1/responses`
- WebSocket upgrade on `/v1/responses`
- `GET /v1/models`
- `POST /v1/memories/trace_summarize`
- `POST /v1/responses/compact` when the installed Codex build sends it to the custom provider

Explicitly unsupported model-provider routes must fail closed before account selection. The router must not forward an unknown path merely because local auth succeeded. A new path becomes supported only after the spec names its route contract and proof requirements.

The router must preserve methods, paths, queries, request bodies, response status, response bodies, streaming event order, WebSocket frames, and Codex metadata headers unless the spec explicitly permits a change.

Allowed request changes:

- reject missing or invalid local router auth before account selection
- strip `X-Codex-Router-Token` before forwarding upstream
- strip hop-by-hop headers
- inject selected upstream account auth
- add upstream headers required by the OpenAI/ChatGPT API
- add correlation metadata that is safe, redacted, and accepted by upstream

Allowed response changes:

- strip hop-by-hop headers
- return `x-codex-turn-state` as an opaque signed router envelope when same-turn affinity is required
- preserve standard `ETag` semantics on `/v1/models`
- preserve `x-models-etag` from Responses WebSocket handshake responses when upstream provides it
- map local router auth failures to local HTTP errors before upstream connection

The router must accept and preserve unknown Codex request fields. Unknown fields are Codex-owned protocol evolution, not router-owned validation errors.

Current Codex also contains Realtime/WebRTC model-provider routes such as `/v1/realtime/calls` and `/v1/realtime`. These are out of scope for v1 and must fail closed before account selection. Supporting Realtime requires a separate route design because its media/session semantics are not the Responses proxy contract.

## Routing Granularity

HTTP/SSE requests are selected per request.

WebSocket connections are selected once, but not from handshake metadata alone. The router accepts the local upgrade after local auth, waits for the first client `response.create` frame, reads only bounded routing metadata from that frame, selects the upstream account, opens the upstream WebSocket, forwards the exact first frame unchanged, and then pins the connection to that account until close. The router must not claim per-message account switching in v1.

Same-turn affinity may use `x-codex-turn-state`, but the value returned to Codex is router-owned state, not raw upstream state. When upstream also returns turn state, the router value must be a signed envelope that can carry both the router account pin and the upstream token. On subsequent same-turn requests, the router validates the envelope, uses the router pin for account selection, forwards only the upstream token to upstream when needed, and never leaks router internals upstream. The envelope must not be replayed across turns, users, router instances with different signing keys, or unrelated Codex sessions.

Previous-response affinity is correctness-critical when Codex sends `previous_response_id` or any upstream state reference that must remain on the same account. The selector must prefer the owning account for such continuation traffic unless that account is disabled or unauthenticated. If the owning account is unavailable, the router fails clearly rather than silently replaying state against a different account.

## Account And Quota Model

An account is a router-owned upstream identity with:

- opaque local account id
- redacted human hint, such as email hash or display label
- enabled/disabled state
- OAuth credential reference
- quota snapshot
- active reservations
- optional affinity ownership records

Quota snapshots are refreshed in the background. Request-time routing reads existing snapshots and must not block the accept loop on broad quota refresh. A request may trigger a narrow refresh only when that is bounded, account-scoped, and outside the committed upstream response path.

Quota state is advisory until it is used as a precommit gate. The router should prefer fresh live quota data, then persisted snapshots, then unknown quota state. Unknown quota must not be treated as free capacity when known healthy accounts exist.

The selector should balance accounts using quota headroom and active reservations, not naive modulo round-robin. The target behavior is weighted deficit round robin over eligible accounts:

- accounts with more remaining quota receive more turns
- accounts with active reservations have reduced immediate headroom
- disabled, unauthenticated, expired, or explicitly exhausted accounts are ineligible
- stale snapshots are penalized
- same-turn and previous-response affinity can override balance when correctness requires it

The router should rotate before commit when the selected account is known to be exhausted, unauthorized, expired, or explicitly rejected for quota/auth reasons. It must not rotate on upstream 5xx, overload, timeout, DNS failure, connection reset, slow stream, client cancellation, or post-commit stream failure.

## Routing State Machine

HTTP/SSE request state:

```text
receive request
  -> validate local router auth
  -> classify route kind
  -> read bounded routing metadata
  -> resolve required affinity
  -> load quota snapshots and account states
  -> select eligible account
  -> create reservation
  -> strip local auth and hop-by-hop headers
  -> inject upstream account auth
  -> forward request unchanged
  -> before commit: classify explicit auth/quota rejection
       -> if retryable by account rotation, release reservation and select another eligible account
       -> otherwise commit response
  -> after commit: stream/forward unchanged
  -> finalize reservation and audit decision
```

WebSocket connection state:

```text
receive upgrade
  -> validate local router auth
  -> classify route kind
  -> accept local WebSocket
  -> wait for first response.create frame
  -> read bounded routing metadata from the first frame
  -> resolve required affinity
  -> load quota snapshots and account states
  -> select eligible account
  -> create connection reservation
  -> strip local auth and hop-by-hop headers from upstream handshake
  -> inject upstream account auth into upstream handshake
  -> open upstream WebSocket
  -> forward first frame unchanged
  -> return upgrade response
  -> forward frames unchanged except bounded routing/error observation
  -> close both sides when either side closes
  -> finalize reservation and audit decision
```

The router is allowed to parse only bounded routing/error metadata required by this state machine. It is not allowed to inspect prompts, tool arguments, images, files, or memory trace contents for policy decisions.

## Secret Storage

The router owns OAuth credentials. It must not depend on Codex `auth.json` as its runtime source of truth.

The secret-store boundary must support:

- storing refresh tokens and access tokens without accidental `Debug`, JSON, or log exposure
- refreshing access tokens in the background
- single-flight refresh leases to prevent refresh storms
- account-scoped credential updates
- explicit account login, enable, disable, and logout operations

Preferred real backend: OS keychain or 1Password-backed adapter.

Allowed deterministic backend: hardened file store behind the same trait, with private router root permissions, private file permissions, atomic temp-write plus rename, symlink protection, and parent-directory validation.

The local router bearer token is a secret. It must be generated, stored, rotated, and redacted like OAuth-adjacent material. Loopback binding is not authentication.

## Local Auth And Audit Threat Model

The default local-auth carrier is `env_http_headers = { "X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN" }` in the Codex custom provider profile. Codex omits this header when the environment variable is missing or empty, so the router must treat a missing header as unauthenticated and reject before account selection.

The router must provide read-only activation output:

- profile text for the Codex custom provider
- a shell-safe command to export `CODEX_ROUTER_TOKEN` for the current shell
- a doctor check that reports whether the token env var is present without printing the token
- a dry-run profile write preview

Any command that writes `~/.codex/<profile>.config.toml` must be explicit, must have a dry-run mode, and must require an approval flag such as `--approve-codex-home-write`. It must not be part of normal serving or login.

Token rotation invalidates the old local bearer for new HTTP requests and new WebSocket handshakes. Existing WebSocket connections authenticated with the old token must be closed by the router during rotation with a local close reason that does not include secrets. Existing HTTP/SSE responses already committed are allowed to finish, but their reservations must be finalized under the old token generation.

Audit events must use a positive schema allowlist. Allowed fields are:

- request id
- route kind
- transport kind
- local auth result without token value
- redacted account id or account hash
- quota snapshot age band
- quota headroom band
- reservation id
- affinity key hash
- decision reason
- precommit rotation count
- response commit state
- error class without body text

Audit events must not carry arbitrary JSON details, request bodies, response bodies, prompts, tool arguments, image data, raw memory traces, auth headers, access tokens, refresh tokens, local bearer tokens, raw account emails, or upstream proxy credentials.

## Configuration

Router config is limited to router-owned concerns:

- listen address, defaulting to loopback
- local router auth settings
- account registry metadata
- secret-store backend selection
- quota refresh intervals and freshness thresholds
- selection policy weights and staleness penalties
- audit log destination and redaction policy

Forbidden config categories:

- Codex install path
- Codex home scanning
- Codex session/history/log/plugin/hook/MCP paths
- shared Codex home
- symlink or repair behavior
- Codex command-line rewriting
- provider timeout gates
- stream idle timeout policy
- retry policy
- health or circuit-breaker policy
- smart context
- prompt rewriting
- provider abstraction beyond the OpenAI/ChatGPT OAuth account family
- Claude, Gemini, Anthropic, or generic multi-provider routing
- Realtime/WebRTC routing
- Prodex gateway admin, virtual-key, billing, metrics, guardrail, SCIM, SSO, tenant, Redis, Postgres, OpenAPI, or route-strategy surfaces

Unknown config fields must be denied by default.

## Architecture Boundaries

The intended Rust workspace is small and boundary-first:

- `codex-router-cli`: command parsing and process bootstrap only
- `codex-router-core`: ids, config, errors, redaction types, audit event types
- `codex-router-auth`: OpenAI/ChatGPT OAuth login and refresh protocol
- `codex-router-secret-store`: credential storage traits and backends
- `codex-router-quota`: quota fetch, quota windows, snapshots, background refresh
- `codex-router-selection`: account eligibility, weighted selection, reservations, affinity
- `codex-router-proxy`: HTTP/SSE and WebSocket forwarding
- `codex-router-test-support`: mock upstream, fixtures, transcript helpers

Dependency direction must keep concerns separate:

- proxy may ask selection for an account decision; selection must not know proxy internals
- quota may read credentials through auth/secret traits; quota must not know proxy internals
- auth must not know proxy internals
- secret-store must not know proxy, quota, or Codex protocol details
- core may be depended on by all crates, but must not become the whole app

## Source-Mining Policy

Codex is the reference for Codex provider behavior, profile configuration, request headers, WebSocket behavior, model catalog shape, memories, compact behavior, and Rust quality standards.

Prodex is source-mining material only. The project may port ideas or tests from:

- OAuth parsing and refresh fixtures
- quota response models and quota-window tests
- refresh-lease coordination
- secret-store trait shape and hardened file-store ideas
- HTTP/SSE header filtering and response tapping patterns
- WebSocket proxy mechanics
- narrow provider conformance fixtures that describe OpenAI/ChatGPT-compatible request preservation

The project must not port:

- shared Codex filesystem management
- session/history repair or merging
- Codex home symlink/copy/migration behavior
- Codex launcher/profile argument rewriting
- Claude, Gemini, Anthropic, or generic runtime-provider paths
- smart context or prompt rewriting
- overload/5xx retry policy
- provider health gates or circuit breakers
- Prodex profile selection state as the account model
- Prodex `provider-core` as an architecture
- Prodex gateway admin, virtual-key, billing, metrics, guardrail, SCIM, SSO, tenant, Redis, Postgres, OpenAPI, or route-strategy surfaces
- cross-provider adapter contracts

## Rust Quality Standards

The project must follow the parts of Codex's Rust discipline that fit a small repo:

- Rust 2024
- checked-in toolchain
- workspace-level dependencies and lints
- `rustfmt` with item-level import granularity
- `cargo clippy --workspace --all-targets -- -D warnings`
- no production `unwrap` or `expect`
- async lock guards must not cross `.await`
- `thiserror` for domain errors; `anyhow` only at CLI/bootstrap edges
- newtypes for account ids, route ids, reservation ids, and affinity keys
- secret wrapper types that do not expose raw values through debug, display, serialization, panic, or logs
- `cargo nextest` as the normal test runner
- `cargo deny`, `cargo audit`, and dependency hygiene checks in CI

The implementation plan must define exact files, commands, and proof gates for these standards before code work starts. Any deferral must be explicit in the plan review.

The project must not copy Codex's large-repo machinery such as Bazel, custom CI sharding, release matrices, or archive-backed test infrastructure until scale justifies it.

## Testing And Proof Requirements

Unit proof:

- config accepts intended fields and rejects forbidden/unknown fields
- selector eligibility, weighted deficit behavior, staleness penalties, reservations, and affinity
- quota window classification
- auth expiry and refresh-needed classification
- redaction wrappers and audit event serialization
- no production `unwrap`/`expect` lint escapes
- signed turn-state envelope encode/decode, upstream token preservation, and replay rejection
- audit schema rejects arbitrary fields and secret-bearing values

Integration proof:

- file secret-store permissions, symlink refusal, atomic write behavior, and parent-directory checks
- refresh lease owner/follower behavior and stale-lock recovery
- mock quota refresh with persisted snapshot fallback
- local auth rejects missing or bad tokens before account selection
- local token rotation rejects old tokens and closes old-token WebSockets
- profile print and profile write dry-run never mutate `~/.codex`
- profile apply requires explicit approval and writes only the named profile file
- audit logs contain allowed fields and exclude secrets

Protocol proof:

- HTTP/SSE method, path, query, body, status, headers, and event stream are preserved
- local router token is stripped upstream
- selected upstream auth is injected exactly once
- explicit precommit auth/quota failure can rotate
- timeout, 5xx, overload, DNS, reset, cancellation, and post-commit failures do not rotate
- WebSocket handshake validates local auth before upstream connection
- WebSocket first-frame routing reads only bounded metadata before opening upstream
- WebSocket first frame and later frames are forwarded unchanged
- WebSocket account selection is connection-scoped
- `/v1/models`, standard `ETag`, WebSocket `x-models-etag`, `/v1/memories/trace_summarize`, and `/v1/responses/compact` are covered
- unsupported paths, including Realtime/WebRTC paths, fail closed before account selection

Smoke proof:

- installed Codex can run through a router profile against a mock upstream
- installed Codex can execute a runtime command with `--profile codex-router`
  against the mock router
- smoke captures the installed Codex version and the profile used
- a temp `CODEX_HOME` or equivalent isolated profile fixture is used
- `CODEX_ROUTER_TOKEN` is injected for the smoke without printing it
- HTTP/SSE and WebSocket modes are each exercised when enabled
- mock upstream transcript assertions prove header stripping, upstream auth injection, and body/frame preservation
- a hostile local request without the router token never opens an upstream connection

Gated live proof:

- real OAuth login, real quota fetch, real upstream account rotation, and real quota pooling tests require explicit approval before use
- live tests must redact account labels, tokens, request bodies, response bodies, prompts, memory traces, and tool arguments

Plan-create must turn this section into a requirements/proof matrix before implementation. Each matrix row must include requirement id, proof layer, fixture or mock, command, expected observation, source reference, proof owner, and stale-proof guard. Mandatory rows include repo setup standards, source provenance, HTTP/SSE preservation, WebSocket first-frame routing when enabled, turn-state envelope, local-token lifecycle, profile write gating, installed-Codex mock smoke, hostile-token smoke, and gated live OAuth/quota proof.

## Security Requirements

Assets:

- OAuth refresh tokens
- access tokens
- router bearer token
- account ids and redacted account labels
- quota snapshots
- affinity pins
- routing decisions
- audit logs
- transient Codex payloads

Required protections:

- bind to loopback by default
- require local bearer auth for HTTP and WebSocket
- reject unauthenticated traffic before account selection
- never forward local router auth upstream
- never log auth headers, tokens, prompts, tool arguments, image data, request bodies, response bodies, raw memory traces, or raw account emails
- redact account hints in audit logs
- fail closed on ambiguous credential corruption or concurrent refresh conflicts
- do not expose non-loopback listener mode in v1

Security non-goals:

- defending against a fully compromised same-user account
- content DLP
- remote multi-user service operation
- Codex sandboxing
- MCP, hook, plugin, session, or install management
- Realtime/WebRTC proxying

## Compatibility Matrix

Required v1 compatibility:

- custom Codex provider profile
- HTTP/SSE `/v1/responses`
- WebSocket `/v1/responses` when enabled
- `/v1/models`
- standard `ETag` on `/v1/models`
- WebSocket `x-models-etag` when upstream provides it
- `x-codex-turn-state`
- `/v1/memories/trace_summarize`
- `/v1/responses/compact` compatibility if the installed Codex build sends it to the custom provider; current Codex may not send remote compact for a custom provider named `Codex Router`

Explicitly out of scope for v1:

- `chatgpt_base_url` routing
- Codex native auth replacement
- repo-local `.codex/config.toml` provider activation
- remote router deployment
- generic OpenAI-compatible clients as primary target
- non-Codex tools
- multi-provider routing
- Realtime/WebRTC routes such as `/v1/realtime/calls` and `/v1/realtime`
- Prodex gateway/provider-core architecture

## Open Spec Questions

1. Should v1 ship with WebSocket enabled immediately, or should the first implementation prove HTTP/SSE request-scoped routing before enabling WebSocket?
2. Which real secret backend is preferred first: macOS Keychain, 1Password developer environment, or hardened file store with a migration path?
3. Should `/v1/responses/compact` be hard-required from day one, or required once an installed-Codex smoke proves the custom-provider path uses it?
4. Should local auth stay on `env_http_headers`, or should a later version use Codex command-backed provider auth with a different local-auth header contract?
5. What explicit user approval language is required before live multi-account quota pooling tests?
