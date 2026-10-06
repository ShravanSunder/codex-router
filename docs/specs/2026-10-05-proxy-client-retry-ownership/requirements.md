# Proxy retry ownership: requirements

Router users need the native client to receive provider overload failures faithfully so its existing retry behavior remains authoritative. The owner explicitly limits proxy policy to account selection, stickiness and quota routing, and authorizes removal of generic overload intervention.

| Need | Authority | Outcome | Priority |
| --- | --- | --- | --- |
| U1: Native clients own generic overload retry and recovery | Authorized: owner's 2026-10-05 proxy boundary and repair request | Provider overload reaches the client faithfully, without Router retry guidance or overload-driven disconnect | Required by owner |
| U2: Router owns account pool decisions invisible to the client | Authorized: owner's quota-only routing explanation | Genuine quota routing and account stickiness continue; overload cannot exhaust or rotate an account | Required by owner |
| U3: Repair must be tested and independently reviewed | Authorized: owner's Lead-validation-then-independent-review instruction | Actual proxy behavior is proved before independent Opus 5.5 implementation review | Required by owner |

This first repair removes the confirmed OpenAI WebSocket capacity policy. Claude HTTP 529 already passes through and must retain that behavior. Quota-reset jitter and quota reconnect wire semantics are outside this bounded repair; their disposition remains separate. There is no change to credentials, production configuration, native client retry budgets or entire-turn recovery. The reported Claude incident's actual retry count/cause is unverified.

The consumer journey is simple: client sends request, provider returns overload, client receives that overload, client chooses whether and when to send another request. Router remains responsible for quota-based eligibility on that request.

Continue to [Specification](specification.md) and [Program Design](program-design.md).
