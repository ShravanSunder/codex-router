# codex-router Account-Router Law Implementation Review Packet

Date: 2026-06-25
Mode: files / adversarial implementation review
review_class: source-backed + risk-triggered
source_backed_verdict_attempted: true
whole-source-trace: required

## Accepted Request

Review the current `codex-router` implementation against the product law:

- `codex-router` is an account router.
- It owns only account/OAuth/local-auth routing responsibilities:
  - local auth gate for the router
  - route classification only to choose an account and credential
  - upstream account selection
  - upstream OAuth credential injection
  - quota/state reads needed for account selection
  - bounded affinity metadata needed to keep Codex continuations on the same upstream account
- Codex owns everything else:
  - prompts, messages, tools, files, images, memory payloads
  - retries, fallback, WebSocket lifecycle semantics
  - protocol metadata not needed for account routing
  - request and response payload interpretation
  - WebSocket protocol behavior
- Do not add or preserve extra router validation except what is strictly needed for local auth, account selection, OAuth credential injection, route support/fail-closed, and bounded affinity.

Immediate live symptom to explain:

- Server logs repeated:
  `codex-router loopback connection failed: websocket closed before upstream open: FirstFrameTooLarge`
- User also observed clients attached to unexpected accounts. Treat active client counts and selection/affinity behavior as part of this review.

## Source Spec

Primary source:

- `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`

Critical anchors:

- lines 15-35: pure proxy/product law
- lines 73-128: required stack and no hand-rolled production protocol stack
- lines 168-228: async WebSocket pure proxy, two-phase routing, no per-message switching
- lines 229-266: no hidden buffering or protocol rewriting
- lines 467-529: full issue-closure proof expectations

Supporting plan:

- `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`

Critical anchors:

- lines 376-455: WebSocket accept, first frame, pre-upstream contract
- lines 531-545: structural guardrails
- lines 790-842: pass-through and guardrail proof rows

## Implementation Scope To Review

Primary files:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-cli/src/*.rs` only where it affects serve/quota/session UX claims
- `Cargo.toml`
- `crates/codex-router-proxy/Cargo.toml`

Useful current observations:

- `Cargo.toml` includes `hyper`, `hyper-tungstenite`, and `tokio-tungstenite`.
- `crates/codex-router-proxy/src/server.rs` release WebSocket path constructs:
  `WebSocketProtocolRouter::new(FirstFramePolicy::new(1024 * 1024))`.
- `crates/codex-router-proxy/src/websocket.rs` has:
  - `FirstFramePolicy { max_first_frame_bytes }`
  - `WebSocketCloseReason::FirstFrameTooLarge`
  - `validate_first_frame` rejects text frames larger than the policy before account selection/upstream open.
- `validate_first_frame` parses the whole first frame as `serde_json::Value` and checks for forbidden top-level auth carriers.
- `AuthenticatedWebSocketRouter` and `AsyncAuthenticatedWebSocketRouter` convert the whole validated first frame into `HttpProxyRequest::with_body(first_frame_bytes)` for selection.

## Non-Goals

- Do not propose disabling WebSockets.
- Do not propose changing Codex.
- Do not propose broad CLI cleanup unless it directly affects the live serve/account-routing law.
- Do not propose mid-message or per-message WebSocket account switching.
- Do not add router interpretation of Codex payloads.
- Do not trust previous proof claims; inspect source and tests.

## Review Questions

1. Where does current implementation violate the account-router law by validating, parsing, buffering, truncating, synthesizing, or controlling behavior that belongs to Codex?
2. Is `FirstFrameTooLarge` a valid account-router boundary or an invented router payload policy? If invalid, what is the smallest correct fix and proof?
3. Is first-frame JSON parsing too broad? Can the router extract only minimal bounded account-routing/affinity metadata without requiring the whole frame to parse?
4. Are active-client leases and account selection causing new Codex sessions to attach to accounts that should be held/reserved/exhausted? Review selection, active-load, affinity, and quota-state interaction.
5. Does the release `serve` path truly use Hyper + `tokio-tungstenite` and avoid blocking/manual WebSocket protocol ownership?
6. Are there tests or proof gates claiming pass-through while still allowing payload validation or truncation?
7. What focused test matrix is required before any “fixed” claim?

## Output Format

Return candidate findings only. For each finding:

- severity: blocker | important | follow-up | nit
- title:
- evidence: exact file:line or spec/plan line
- scenario:
- smallest_fix:
- proof:
- confidence: high | medium | low

Also include:

- lane name
- no findings, if true
- remaining uncertainty
- files inspected

Do not edit files. Do not stage, commit, or run formatters.
