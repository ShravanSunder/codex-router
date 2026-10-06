# Transparent overload forwarding

Governing needs: [Requirements](requirements.md). Structural realization: [Program Design](program-design.md).

Entities: E1 is a client/provider WebSocket connection pair, whose lifetime ends with transport closure or an existing authorized shutdown. E2 is a provider error frame, identified by its actual frame payload, including envelope and error code. E3 is the account selected for E1. E4 is a client request: a separately submitted frame, not a Router-replayed turn.

| Obligation | Need | Observable behavior and proof |
| --- | --- | --- |
| S1 | U1 | Forward `server_is_overloaded` and `slow_down` frames unchanged, including `error` and `response.failed` envelopes. No synthetic rate-limit payload, delay recommendation or Close solely for overload. Actual pump tests compare original frame payloads. |
| S2 | U1, U2 | After a terminal overload (`response.failed` or `error` with a non-success numeric status), another explicitly submitted client request traverses E1 to the same upstream. No automatic proxy replay. Actual pump tests observe that request and ordinary upstream reply, then close deliberately. |
| S3 | U2 | Overload does not mark E3 quota-exhausted, select an alternative or redirect account affinity. Existing same-account owner recording and pin renewal continue. Classification still distinguishes overload from quota even when code/type fields mix capacity and quota tokens. Tests inspect observable quota/selection records. |
| S4 | U2 | Existing genuine quota/floor/account admission/reservation/auth/Close behavior remains. Real quota/floor tests retain both pump completion orders, deferred write readiness, forced shutdown and resource cleanup proof. |
| S5 | U1, U3 | Claude HTTP 529 remains pass-through, including status/body and no quota/auth retry. Existing server-pipeline proof remains green. |

The rule is independent of prior overload count and thread-id metadata. There is no overload retry history or expiry policy to expose. Existing genuine transport failure, client Close, revocation, session shutdown and quota/floor closure remain valid reasons to terminate E1.

S1 also applies to statusless error envelopes. Their forwarding does not invent a terminal event or change existing turn-admission semantics; a later actual terminal event releases the turn. This bounded repair preserves that existing distinction.

Acceptance requires intended-red evidence at the actual forwarding boundary, green regression and preservation checks, applicable format/check/lint gates, Lead validation and independent implementation review. An in-memory transport may stand in for a provider network while exercising actual pumps; it cannot establish real provider incident behavior or native client retry timing.
