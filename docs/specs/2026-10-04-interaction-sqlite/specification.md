# Typed interaction SQLite specification

Governing requirements: [requirements.md](requirements.md).

An **E1 typed interaction** is one Approval, RefusedApproval or Question, identified by its request id. Its requester, approver, request/refusal and variant-specific state keep the existing domain meanings. **E2 creation instant** is the UTC time assigned when a new interaction is durably recorded; settlement does not reset it. **E3 acknowledged history view** is the last successfully validated/published store state. Listings retain that view until a refreshing operation observes a newer committed revision; it is not proof of provider consumption.

| Contract | Need | Observable obligation and proof |
|---|---|---|
| R1 Persistence | U1 | Successful record/settlement commits survive independent SQLite reopen with exact typed content and immutable E2. Test actual files and terminal rows. |
| R2 SQLite-only cutover | U2 | Never inspect/read/import/hash/write/delete interaction-history.json. Its absence, valid contents, malformed contents, changed contents or a non-file path cannot affect startup/reopen. Fresh history contains zero rows; later SQLite records persist. Test through owning broker startup, independent SQL count and unchanged file bytes. |
| R3 Validation and ownership | U4 | Invalid stored identity, variant, request id, answer, timestamp, duplicate JSON fields, storage class, schema or revision fails unavailable. Unknown/corrupt databases remain unchanged. No coercion, repair, compatibility adapter or JSON fallback. |
| R4 Settlement semantics | U3 | Preserve actor/option checks and one terminal winner. Questions validate then send the response before history persistence; approvals persist the decision before sending. Write failure must not send an approval. A sent Question plus failed history write remains partial success without automatic resend. Test broker channels with actual SQLite failure. |
| R5 Startup and retention | U3 | Owning cold startup explicitly cancels pending records as hostRestarted. Storage-only reopen never declares records orphaned. Prune strictly older than 30 days from E2, at nanosecond precision, bounded batches 1..500, ordered oldest then request id. Test durable deletion and failed reconciliation. |
| R6 Transactions and cache | U4 | Serialize writes and validate current authoritative state before mutation. Concurrent storage writers preserve unrelated records. An aborted call after commit may leave E3 stale; the next refreshing operation recovers without replaying the old mutation/provider response. Test two connections and commit-before-cache cancellation. |
| R7 Proof and delivery | U5 | Run real storage/MCP/broker tests and quality checks; obtain independent implementation review and required native debug settlement proof before PR readiness. Setup-only/model-free startup is not settlement proof. |

R2 intentionally removes old typed JSON data from the visible history; files are preserved for owner-controlled future handling. Starting an older binary would observe its old JSON state rather than current SQLite history. This work neither merges nor exports those histories. The separate legacy approval file remains supported under its existing contract.
