# R5 Lane Receipt: Security + WebSocket

Agent: Wegener
Status: answered
Verdict: needs revision

Coverage: read the full 1014-line spec, R4 ledger, and cheap WebSocket/auth
code anchors.

Candidate findings:

- Blocker: previous-response affinity is fail-closed in prose, but exact
  metadata extraction is not defined.
- Important: zero-side-effect proof omits oversized and timed-out first-frame
  failures.
- Question: WebSocket local-auth and unsupported-path pre-upgrade rejection
  semantics are not stated.

Parent disposition: accepted the blocker and important finding; resolved the
question in the spec by requiring pre-upgrade rejection for local auth and
unsupported paths.
