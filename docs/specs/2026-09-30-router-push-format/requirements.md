# Router push format — Requirements

## Purpose and boundary

Router pushes text into agent sessions: direct messages, wakes, schedule runs, approval and question
notices, and thread activity. Today these pushes carry JSON envelopes ("Self-declared sender: {…}",
"Intended recipient: {…}"), raw batch dumps, separate end records, and whole message bodies. They are ugly
and they fill agent context. The owner is "trying to prevent this" (2026-09-30).

Every push becomes a short, readable notice. The agent fetches the full content with one easy command.
Router stores what it points at.

In scope:
- the shape of every Router push into a session;
- where the full content is stored, and for how long;
- how an agent fetches it and replies;
- the link format, which must also serve future cross-machine delivery.

Out of scope:
- cross-machine delivery itself;
- board, topic and thread semantics;
- thread subscriptions timing, which has its own design.

Authority: owner decisions in the codex-router Main session on 2026-09-30, recorded on the Agent Router
board (Thread subscriptions coordination root 01a0eae5, messages 01a0f103 and 01a0f168).

## Who is affected

| Class | Job | Current pain |
| --- | --- | --- |
| Receiving agent (Sidekick, Worker, Main) | acts on messages while working | JSON headers and full bodies flood its context; long receipts repeat on the thread and in DMs |
| Owner | reads agent sessions and coordinates many agents | pushes are "very ugly and take space" |
| Sending agent | reports and asks | its reply goes to whichever sender was latest (guessed); a message to a closed session fails |

## Authorized needs

| ID | Need | Authority | Priority |
| --- | --- | --- | --- |
| U1 | Every Router push into a session is compact. It has a one-line header, a preview of at most **100 characters** of the content (header and link not counted), and a link to the full content. It contains no JSON envelopes. | owner: "100 char not including other stuff like links" | must |
| U2 | The full content is stored by Router and easy to fetch with one command, using the link. | owner: "it should be easy for them to check and we store" | must |
| U3 | Stored push content is kept for **30 days**, then deleted, for everything. | owner: "lets make it 30days gone for everything" | must |
| U4 | Headers use emojis by default: a kind emoji plus the sender's role emoji where known. | owner: "i like use of emojis as default" | must |
| U5 | The link identifies the machine (the Router instance) and the store and item, so a future cross-machine fetch works without changing the format. | owner: "user needs to know which machine and where they get the data… for inter-machine later" | must |
| U6 | Direct messages are generic Router messages. They never create board projects, topics or threads. | owner: "all sends can be generic then not thread. i dont like the idea of polluting the board" | must |
| U7 | Automation pushes (wakes, schedule runs, approvals and questions) follow the same notice-plus-fetch behaviour. | owner: "we should also do this for automation sends too… same behaviour" | must |
| U8 | Thread-activity pushes stay neutral: no preview of peer-authored text. | owner decision 01a0f103 (neutral notice) | must |
| U9 | A reply answers a specific message, not whoever wrote last. | owner agreed with the reply-by-id shape | should |
| U10 | A direct message to a session that isn't running is held and delivered when the session is back, not failed. | Main proposal that the owner discussed without objection; to confirm | should |
| U11 | Only the sender, the recipient and the owner can `show` a direct message. A link is not a capability for anyone else. | owner: "yes to a" (2026-09-30) | must |
| U12 | The owner's own messages arrive in full text, not previewed, but only when Router can prove they come from the owner. Agents must not be able to fake the owner. | owner: "sounds good but anyone can fake me no?" (2026-09-30) | must |
| U13 | The sender shown on a push is the identity the caller's client reports (its harness session), not a free `--from`. Real stamping (authenticated caller context) is a **separate follow-up design** covering per-session tokens for Codex and ACP, a Claude wrapper or process-id tracing, and owner verification. | owner chose A (2026-09-30), after the caller-identity map at tmp/design-workflows/2026-09-30-router-push-format/caller-identity-map.md; "fix that later, keep track" | must (reported identity); stamping deferred and tracked |

**Owner decisions after the first draft (2026-09-30):**
- Fresh-session schedule runs carry their full instruction (answer to the fresh-runs question). The same
  task-input boundary covers the summary worker's fresh read-only thread.
- `--human-user` stays self-declared for now and is shown as `🧑 Owner (unverified)`, with no special
  authority. Owner verification (1Password biometric via `op`, working on macOS and Linux) is a follow-up
  design item. **Until it exists, unverified owner sends use the normal compact push. There is no
  full-text exception and no owner bypass for `show`, because an agent could otherwise claim to be the
  owner.** U12's full text and U11's owner access arrive with that follow-up. This is pending the owner's
  confirmation.
- The machine label is the Remote Control name, falling back to the host name.
- #104 merges as is; this design replaces its reply mechanism.

**Lead scope call, recorded and open to owner override:** "30 days for everything" (U3) covers every record
Router stores *as a push*: DMs, wake firings, schedule-run notices, approval and question notices,
subscription notices and their batch ranges, and delivery evidence bodies. Board threads and reusable
definitions (schedules, instruction documents, wake definitions) are not push content and keep their own
lifetimes.

## Limits and non-goals

- Cross-machine delivery is **not** built here. The link format only has to be ready for it.
- Board, topic and thread storage are unchanged. Thread notices already follow the thread-subscriptions
  design; this design only gives them the shared link format.
- No new notification channels (email, OS notifications).
- Hard cutover: the old JSON envelope formats are removed, with no compatibility path.

## Unresolved hypotheses (to settle in the Specification)

- Whether a schedule run that starts a **fresh** session should carry its full instruction instead of a
  notice, since the instruction is the whole task and the context is empty. This is Main's proposal, not
  yet an owner decision.
- Whether the sender identity can be stamped by Router from the caller's session instead of self-declared.
