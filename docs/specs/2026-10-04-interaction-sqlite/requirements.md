# Typed interaction storage migration

The owner requested a baby step: move typed interaction history from its JSON file into `interaction.sqlite`. Approvers, requesters and operators need durable history without a change to approval or Question behavior. This is a storage migration, not the future unified messages/inbox/outbox/lifecycle database.

| Need | Consumer and outcome | Authority / priority |
|---|---|---|
| U1 | Requesters and approvers retain typed approval, refused-approval and Question history in `interaction.sqlite`. | Owner's 2026-10-04 bounded migration commission; authorized, required by owner. |
| U2 | Operators can import existing typed JSON transactionally, reject malformed data and recover without deleting the original file. Legacy history retains its frozen 0.1.38 shape and separate runtime writer. | Same commission; authorized, required by owner. |
| U3 | Requesters and approvers retain actor validation, offered-choice validation, settlement, cancellation, response ordering and 30-day creation-time retention. | Commission's explicit preservation constraints; authorized, required by owner. |
| U4 | Operators can distinguish a safe cutover/restart from old-writer divergence or unsafe rollback. No automatic destructive recovery. | Commission's old-writer/restart/rollback and no-deletion constraints; authorized, required by owner. |
| U5 | Owner receives real SQLite and broker proof, independent review, coherent local checkpoints with signing preferred and this lane's merge-ready, non-draft PR, left unmerged. | Original commission plus latest explicit merge-ready delivery criterion; authorized, required by owner. |

There is no new product interface, authorization policy, provider transcript, credential, global setting or legal-acceptance behavior. Automation and message storage stay outside this task. The fixes design's Question caller-work lifetime is separately owned by the fixes Lead; this document's U3 preservation obligation remains in scope. Host quiescence/handover is separately owned by the Host Lead. The owner's later delivery criterion permits this lane's merge-ready, non-draft PR, left unmerged; merge, release and production restart remain unauthorized.

| Today | Requested outcome | Preserved boundary |
|---|---|---|
| Typed history uses whole-file JSON writes and rename. | Typed history has SQLite transactional persistence. | Same typed broker and domain records. |
| Startup validates history and cancels orphaned pending records. | Import/reopen validates durable state and retains ordinary startup cancellation. | No new answer, replay or lifetime policy. |
| Legacy approval history has a frozen 0.1.38 record shape and its existing writer. | Typed import, reconcile, prune and persistence neither import, modify nor delete that file; ordinary legacy activity retains its own writer. | Existing legacy readers and writers. |

See [Specification](specification.md) for observable obligations and [Program Design](program-design.md) for their realization.

The latest owner-supplied AGENTS signing policy permits a per-commit unsigned fallback after two failed signing attempts. Keep hooks and global signing configuration intact, report each failure, and label unsigned checkpoints explicitly. This operational fallback does not change the storage contract.
