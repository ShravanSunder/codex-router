Research Ledger
═══════════════

Question:
Do codex-router's async WebSocket assumptions match real Codex Responses
WebSocket behavior, or did the router invent protocol invariants that break
live Codex clients?

Mode:
research-only / design-input

Non-goals:
- No implementation changes in this ledger.
- No new subagent fan-out until the local invariant audit is stable.
- No OAuth/keychain/session-picker work.

Sources:
- Local Codex source:
  `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs`
  used as the primary behavior contract for Codex CLI.
- Current codex-router source:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router`
  used to identify mismatches.
- DeepWiki answer for `openai/codex` was used only as an initial pointer; all
  accepted findings below are anchored in local source.

Lane Summary:
- `codex-ws-source-invariants` (`019efbeb-2638-75a2-a737-e830323464c0`,
  Copernicus): read-only primary Codex source audit for WebSocket invariants
  that an account-router proxy must preserve.
- `router-invariant-drift` (`019efbeb-2a2c-7182-94bc-86c06c715904`,
  Averroes): read-only current router/spec/plan audit for assumptions that
  violate the account-router/pass-through model.
- `proof-matrix-account-router-adequacy`
  (`019efbeb-2ebd-7733-8f7e-bb6c21406b58`, Halley): read-only proof-matrix
  audit for rows that could pass while Codex preconnect or pass-through
  behavior remains broken.

Parent Reduction Of Lane Findings:
- Accepted from `codex-ws-source-invariants`: Codex preconnect is handshake-only
  and must not trigger synthetic router request frames; warmup is a real
  `response.create` with `generate=false` that must pass upstream; WS payloads
  are direct serialized request objects; `previous_response_id` and
  `x-codex-turn-state` are Codex-owned semantics that the router must preserve
  but not invent; ping/pong are transport control frames; close before
  `response.completed` is a client-visible error; retry/fallback is Codex-owned.
- Accepted from `router-invariant-drift`: the release path still has active
  account-router drift risks: 250ms pre-upstream first-message timeout, WS
  first-frame validation that requires prompt-bearing payload shape, HTTP full
  request-body buffering, unbounded response affinity scan buffers, synthetic
  `/v1/models`, and release-path WS truncation/provider-event-aware forwarding
  behavior.
- Accepted from `proof-matrix-account-router-adequacy`: the proof matrix needed
  stronger contract-shaped rows for preconnect, pre-upstream control frames,
  HTTP/SSE pass-through, WS pass-through, `/v1/models` upstream pass-through,
  stable-router-PID soak continuity, and structural guardrails for buffering,
  synthetic responses, payload-shape validation, and release WS truncation.
- Parent action: folded these accepted findings into
  `async-router-runtime-spec.md` and `implementation-plan.md` before resuming
  implementation.

Evidence
────────

1. Codex explicitly supports WebSocket preconnect with no request frame.
   class: direct observation
   supports/refutes/complicates: refutes router invariant that a
   `response.create` must arrive shortly after local upgrade
   source:
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/client.rs:1131-1168`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/tests/suite/client_websockets.rs:879-900`
   confidence: high

   Notes:
   `ModelClientSession::preconnect_websocket` says it performs only connection
   setup and never sends prompt payloads. The Codex test
   `responses_websocket_preconnect_runs_when_only_v2_feature_enabled` asserts
   one handshake and zero connection requests immediately after preconnect.
   Therefore an upgraded WebSocket that is temporarily idle before the first
   request is legal Codex behavior.

2. Codex reuses one WebSocket connection for later request frames.
   class: direct observation
   supports/refutes/complicates: supports router requirement to keep idle or
   preconnected sessions alive until the later request or Codex's own timeout
   source:
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/client.rs:225-250`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/tests/suite/client_websockets.rs:343-364`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/tests/suite/client_websockets.rs:708-740`
   confidence: high

   Notes:
   Codex creates a `ModelClientSession` per turn, lazily opens/caches a
   Responses WebSocket, and reuses it for later requests within the turn.
   Tests assert one handshake and one later request after preconnect, including
   cases where request metadata changes after preconnect.

3. Codex v2 startup/request prewarm sends `response.create` with
   `generate=false` and waits for completion before the next request.
   class: direct observation
   supports/refutes/complicates: supports router first data-frame parsing as a
   route-selection boundary once a data frame actually arrives
   source:
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/client.rs:15-19`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/client.rs:1571-1621`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/tests/suite/agent_websocket.rs:70-114`
   confidence: high

   Notes:
   Once Codex sends a request data frame, it is still a
   `response.create`-shaped request. Router can continue selecting on the first
   bounded data request frame, but it cannot assume that request frame arrives
   immediately after handshake.

4. Codex WebSocket implementation handles ping/pong/control frames at the
   transport pump layer and ignores pong; text/binary/close are surfaced to the
   response stream logic.
   class: direct observation
   supports/refutes/complicates: complicates router first-frame validation
   source:
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/responses_websocket.rs:63-126`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/responses_websocket.rs:627-750`
   confidence: high

   Notes:
   Router must preserve WebSocket control-frame behavior. Before upstream is
   open, local ping/pong/close handling must not be confused with an invalid
   first request payload. After upstream is open, both directions should remain
   transparent except for already-specified bounded metadata observation.

5. Codex uses provider `stream_idle_timeout` for send and response wait on an
   established WebSocket request, and has session-scoped fallback to HTTP after
   retry exhaustion.
   class: direct observation
   supports/refutes/complicates: refutes router-owned retry/fallback and
   supports using Codex/provider timing rather than invented local 250ms timing
   source:
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/responses_websocket.rs:627-650`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/codex-api/src/endpoint/responses_websocket.rs:757-775`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/responses_retry.rs:24-45`
   - `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/client.rs:1688-1701`
   confidence: high

   Notes:
   Codex owns fallback. Router closing an otherwise legal preconnected WebSocket
   fabricates a transport failure and pushes Codex into reconnect/fallback paths
   it did not choose.

6. Current router closes upgraded local WebSockets if the first message does
   not arrive within 250ms.
   class: direct observation
   supports/refutes/complicates: root cause candidate for live
   `FirstFrameTimeout` / broken pipe
   source:
   - `crates/codex-router-proxy/src/websocket.rs:1831-1865`
   confidence: high

   Notes:
   This is incompatible with Codex preconnect. A local Codex session can
   legally complete handshake and send no request frame yet.

7. Current router validates the first data frame as text JSON
   `response.create` before upstream open.
   class: direct observation
   supports/refutes/complicates: likely acceptable only for the first data
   request frame; not acceptable as a rule for all pre-upstream frames
   source:
   - `crates/codex-router-proxy/src/websocket.rs:220-270`
   confidence: medium

   Notes:
   This should be reframed as "first bounded request data frame" rather than
   "first WebSocket frame." Control frames and close must be handled according
   to WebSocket semantics before upstream is open.

Synthesis
─────────

supported:
- codex-router may select an account from the first bounded Codex request data
  frame because Codex Responses WebSocket requests are `response.create` shaped.
- codex-router should not implement router-owned retry/fallback/account switch
  after upstream open; Codex owns retry/fallback.

refuted:
- The current 250ms post-upgrade first-frame timeout is not a Codex invariant.
- The current proof that only sends immediate first frames is not sufficient
  compatibility proof.
- "First frame" is too broad. The compatibility contract must distinguish
  WebSocket control/close frames from the first request data frame.

complicated:
- Account selection currently depends on request body, so router cannot open
  upstream with the selected account until the first request data frame arrives.
  That means the pre-upstream local session must be able to stay open while
  waiting for Codex's request frame, subject to shutdown/revocation/resource
  limits aligned with Codex behavior, not a 250ms invented deadline.

unresolved:
- Exact production upper bound for a preconnected-but-idle local WebSocket:
  likely should be configurable and aligned to provider/Codex stream/connect
  timeout, but it must be long enough for Codex preconnect and startup prewarm.
- Whether router should reject long-idle pre-upstream sockets with a clear
  policy close after a large resource bound, or hold until client close/shutdown.
- How to expose this in spec/plan without allowing unbounded resource leaks.

Required spec/plan change
─────────────────────────

The accepted async runtime spec currently says "wait for bounded first
response.create frame." That must be revised to:

- "After local WS upgrade, allow Codex-compatible idle/preconnect state before
  the first request data frame."
- "The router may parse only the first bounded Codex request data frame for
  route/account selection."
- "Control frames before upstream open are handled as WebSocket control frames,
  not invalid first request frames."
- "No release-path short wall-clock first request data frame timeout is allowed.
  Legal Codex preconnect may remain idle before request data until client
  close, router shutdown, or token/session revocation. This goal does not
  introduce a new pre-upstream idle timeout or local resource-policy cap."
- "Proof must include installed Codex or Codex-source-compatible fixture for
  preconnect with zero immediate requests, delayed first request, and reuse of
  the same local WebSocket."

Recommended Next Workflow:
shravan-dev-workflow:plan-creation-swarm after the spec is revised, because
the current plan/proof matrix is now known to be missing a hard compatibility
gate.
