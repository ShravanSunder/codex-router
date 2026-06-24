# Lane: security-reliability

Status: answered
Reasoning effort: high
Security context: applicable

## Source Coverage

- Spec anchors: post-upgrade WebSocket failures, buffering/state/credential
  commit, auth/session revocation/observability, redacted soak evidence,
  guardrail scope, pump side-effect guardrail, security context.
- Current code anchors: CLI serve path, proxy server, WebSocket tunnel,
  HTTP/SSE service, auth resolver, state store, secret store, audit sink, and
  installed-Codex evidence.

## Concrete Plan Constraints

1. Preserve explicit auth modes.
   - Default `serve` remains tokenless loopback.
   - `--require-local-token` remains explicit hardening.
   - Token-required mode rejects before route classification, selection, or
     upstream egress.

2. Local auth material never crosses upstream.
   - Strip local token carriers and hostile upstream-auth canaries.
   - Prove this across HTTP/SSE and WebSocket.

3. WebSocket admission remains bounded and fail-closed.
   - Keep bounded header/first-frame size and timeout contracts.
   - Failures after local accept close locally with redacted reasons and no
     router retry/account switch.

4. Release `serve` has exactly one async runtime.
   - No production blocking tungstenite, `BlockingWebSocketTunnel`, hidden
     feature flag, alternate command, or compatibility serve path.

5. Session registry moves from cloned sockets to async session handles.
   - Track session id, token generation, cancellation handle, close reason.
   - Unregister on normal close, local close, upstream close, revocation,
     upstream-open failure, cancellation, and shutdown.

6. Pump paths are side-effect-light.
   - Pumps may emit bounded in-memory events only.
   - No SQLite, secret-store, durable audit fsync, or blocking waits in
     frame/body forwarding or close-progress paths.
   - Existing inline affinity writes in WebSocket forwarding must move out of
     the pump.

7. Credential refresh remains auth-owned.
   - Runtime must not orchestrate secret write, generation bump, and quota
     invalidation as separate visible steps.
   - Auth/state must preserve cancellation-safe logical commit semantics.

8. Startup/recovery fails closed and stays fast.
   - Unsupported schema or secret-store failures stop startup or the affected
     operation with redacted errors.
   - Broad quota refresh stays off listener bind and first-request acceptance.

## Required Security / Reliability Proof

- tokenless HTTP and WebSocket through real `serve` without local-token state
- token-required missing/empty/old/wrong/smuggled token rejection before
  upstream connection count increments
- first-frame timeout/oversize/malformed/wrong-type failures close locally with
  redacted evidence
- upstream-open failure closes locally without retry/account switch
- token rotation closes stale WebSockets and removes them from registry while
  leaving fresh sessions unaffected
- close-while-pending and blocked-write/backpressure traverse real `serve` and
  prove task termination, registry cleanup, and close reason
- soak proves high-water 3, zero-active-after, all upstream sockets closed, and
  no local socket leaks
- slow SQLite/secret/audit sinks cannot delay forwarding or close completion

## Redaction / Evidence Rules

Durable evidence may include session ids, timestamps, counters, close reasons,
transport, handshake counts, activity counts, account hash or safe tag.

Durable evidence must not include raw prompts, tool payloads, first-frame bodies,
provider responses, tokens, refresh tokens, raw account ids, or account labels.

Every durable artifact must include forbidden-fragment assertions, not only
audit JSONL redaction checks.

## Cleanup / Race Risks

- Current registry registers before upstream connect and has no normal
  unregister path.
- Current WebSocket forwarding writes affinity owner state inline.
- Current close semantics are asymmetric and blocking.
- Current local-token reload watcher polls and races with in-flight sessions;
  async plan must define generation observation and cancellation ordering.

## Split / Replan Triggers

- Split if production blocking tungstenite cannot be removed from the release
  graph in one slice.
- Split if session cleanup cannot be proven through real `serve`.
- Stop and replan if affinity/audit/quota/secret persistence must remain in
  pump hot paths.
- Replan if proof requires live provider OAuth by default.

## Completion Receipt

Answered with concrete security/reliability constraints, proof requirements,
redaction requirements, race risks, and split triggers.

Confidence: high
