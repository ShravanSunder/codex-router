# Messages and automations across machines

Research for [the main file](../2026-10-04-distributed-agent-collaboration.md), 2026-10-04.

**Status labels:**
- **verified:** read in current source at main `7a8cbd89`;
- **inference:** reasoning that hasn't been tested.

## 1. Messages: local-first, store and forward (R8, ADR-4)

A direct message has one writer, never changes once sent, and competes for nothing exclusive. Both of its ends are bound to a machine: the sender's session runs on one, the recipient's on another. So it is local-first, carried by the fabric relay:

```text
 SENDER MACHINE           FABRIC                    RECIPIENT MACHINE
 OUTBOX ──── ship ───► RELAY CUSTODY ── deliver ──► INBOX ── inject ──► session
 (durable intent)      (holds while the             (durable accept)
                        recipient sleeps)
 receipt: custody ─► durable accept ─► native input accepted ─► completed
 same machine: outbox ─► inbox directly (no relay)
```

**The boundary against the board (inference):**

| | Board post | Direct message |
|---|---|---|
| Audience | everyone on the thread | one recipient |
| Decided by | the board authority (ordering, wakes, seat checks) | nothing to decide; it's delivered |
| Placement | server-first | local-first, relayed by the fabric |

**What the interactions lane must design (inference):**
1. **Dedup.** The relay may resend after a dropped connection, so the inbox ignores repeats by message ID plus a payload digest.
2. **Order per pair.** Messages from A to B arrive in send order.
3. **A recipient that moved.** If a session's execution lease moved, the relay routes to its current home. Messages never accepted can be forwarded. Accepted or maybe-accepted messages are reconciled, never resent.
4. **Expiry.** A message to a session that never returns gets a final outcome reported to the sender.
5. **Distinct receipts.** Each receipt stage releases a different resource. "Delivered" doesn't mean read or acted on.
6. **Retention on the fabric.** Payloads are held until delivered; metadata is kept longer.

**Today (verified):**
- Direct messages are stored in the automation store, in `router_pushes` (`crates/automation-storage/migrations/20260930000000_router_pushes.sql`).
- Interrupted sends are marked `outcome_unknown` and never auto-resent (`crates/automation-storage/src/direct_message_recovery.rs:65–88`).
- Other machines' targets are refused (`crates/collaboration-service/src/native_message_dispatch.rs:151`).

## 2. Automations: the automation never moves, only its route

Both projects and sessions can move between machines (R5, R6). The design that survives that (inference):

```text
 automation store (one owner, one clock: decision D2 = fabric)
   definitions · timer cursors · outstanding obligations · run admission
   each automation references its project and its target by stable id
       │ at each attempt: resolve target ─► persist exact session + attempt ─► send
       ▼
 project moved?        automation unaffected (stable project id)
 session moved?        next attempt routes to its new home
 role holder changed?  next attempt goes to the new holder (role targets only)
```

**Why the timer stays put (inference):**
- **Moving a timer is risky.** It means transplanting its watermark, its outstanding obligation and any in-flight attempt, and each transplant risks a double or missed fire.
- **One timer owner means one clock.** With several clocks, a fast one can skip time or expire a wake early, and a successor faithfully preserves the error.
- **The cost:** while the timer's home is unreachable, nothing fires and edits queue.

**Target intent: each automation declares one (inference):**
- **Role:** "nudge whoever is reviewer now". Resolved fresh at each attempt.
- **Fixed:** "ask the reviewer who assessed head H to explain that assessment". Pinned to that person or commission and to the subject revision. Sending it to a successor would change its meaning.

**Rerouting rules (inference):**
1. Resolve the target immediately before admitting an attempt, then persist the exact session, binding and attempt before any I/O.
2. Only an attempt known to be unsubmitted may be re-resolved. An accepted or possibly-accepted attempt is reconciled, never resent to a new home.
3. Triggers for a new route: the session's execution lease moved, the session was recovered elsewhere, or a role changed holder. Sleep, network blips and fabric outages are not triggers; the work waits.

**Safety (inference):**
- **Coalescing belongs to the automation's meaning,** applied at the source. "Check CI status" may collapse missed fires; "review each new commit" must not.
- **The relay never alters a payload.**
- **Dedup uses a business key** (automation, due time, revision) alongside UUIDv7 IDs (R13). It only catches identical repeats. It can't recover a lost obligation, or stop an effect an old holder already sent.
- **One Router's restart must not settle another executor's attempts.**

**Today (verified):**
- A wake folds new fires into its pending delivery while that delivery is pending, retryable, dispatching or uncertain (`crates/automation-storage/src/wakeup_evaluation.rs:83–115`).
- A wake's pending pointer clears on accepted, failed or discarded (`crates/automation-storage/src/delivery_outcomes.rs:81–140`).
- A schedule keeps at most one occupying run and one waiting run (`crates/automation-storage/src/run_inventory.rs:13–35`). New due times fold into the waiting run. The timer watermark advances in the same transaction, and the next due time is computed from now (`crates/automation-storage/src/schedule_evaluation.rs:21–68`).
- Run IDs and occurrence IDs are random UUIDv7. The ID constructors reject other versions (`crates/agent-automation/src/automation_identity.rs:25–40`).
- Wake targets are exact, service-scoped session addresses (`mailbox_deliveries.target_json`).

So today's safety is local SQLite transactions plus "one outstanding obligation per automation". Independent stores on different machines would have no shared admission (inference).

**Open decisions:**
- **D2:** where timers live.
- **D3:** what happens to a fixed-target automation when its session is recovered as a new conversation: deliver it labelled, hold it, or expire it.
- **D5:** whether automations may target a role.
