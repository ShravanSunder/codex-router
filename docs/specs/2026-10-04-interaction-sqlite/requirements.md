# Typed interaction SQLite requirements

Router users need typed approvals, refused approvals and Questions to survive process restarts with transactional updates and explicit storage failures. The owner chose a hard cutover to SQLite and explicitly removed JSON migration/import support: “we just stop supporting json”. Existing typed JSON records therefore do not appear in the new history. Existing files are left untouched.

| Need | Required outcome |
|---|---|
| U1 Durable typed history | New typed interactions and their terminal states persist in interaction.sqlite and survive reopen. |
| U2 One storage authority | No reads, imports, hashes, provenance checks, writes or deletion of interaction-history.json. Fresh SQLite history starts empty regardless of that path's contents or accessibility. |
| U3 Preserve interaction behavior | Approval and Question authorization, offered options, response validation, cancellation, response ordering, cold-start reconciliation and strict 30-day creation-time retention remain observable as before. |
| U4 Safe persistence | Corrupt/foreign databases and invalid stored rows fail explicitly without automatic clearing or fallback. Concurrent mutations and uncertain commit/cache publication cannot lose unrelated rows or replay responses. |
| U5 Evidence and delivery | Focused real SQLite and broker tests, independent implementation review, required debug proof, applicable quality checks and a merge-ready unmerged PR. Incomplete gates are reported as incomplete. |

Only typed interaction persistence changes. The frozen legacy approval-history.json, automation, messages, inbox/outbox, provider transcripts/session files, U3 caller-work lifetime and Host replacement remain separate. No account/security/global setting changes, production replacement or release are authorized by this work. The owner retains control of merge/release.
