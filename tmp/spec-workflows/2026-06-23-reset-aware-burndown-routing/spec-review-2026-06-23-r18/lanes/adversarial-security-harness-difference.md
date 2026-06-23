# R18 Adversarial Security, Harness, And Difference Lane

Verdict: needs revision

Accepted by parent:

- Blocker: WebSocket invalid local auth and unsupported-path proof did not
  require handshake/connect failure or another non-101 local rejection, so a
  post-upgrade close with zero upstream open could falsely satisfy the proof.

What held:

- The spec separated `unsupported_path` from `unsupported_route_band`.
- Affinity/fairness cutover was sharper than current code.
- Security/redaction requirements were explicit.
- HTTP-only proof was no longer enough on paper.

Current-code/spec differences that remain planned implementation work:

- WebSocket ingress currently accepts the local upgrade before auth/path checks.
- Selector affinity currently requires owner membership in weighted candidates
  and records weighted state.
- Installed-Codex transcript fields still need the R18/R19 receipt/redaction
  contract.
- Installed-Codex smoke remains `/v1/responses`-only while route-native proof
  must cover all supported routed APIs.

Receipt:

- Source anchors: spec lines 1-1992, `routes.rs`, `account_selection.rs`,
  `websocket.rs`, `http_sse.rs`, installed-Codex harness.
- Parent reducer wrote this lane summary from the subagent candidate output.
