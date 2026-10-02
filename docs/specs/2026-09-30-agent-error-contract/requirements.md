# Agent error messages — Requirements (revision 2)

## Purpose and boundary

When an agent's Router call fails, the agent should be able to fix its own call or pick another route without
asking the owner. It needs a short explanation and, for input mistakes, which parameter is wrong and what form
it expects.

Owner (2026-09-30): "agents should always have errors and reasons in a succinct way"; "we should at least
provide the reason why"; then, narrowing: "we only need to have short explanation and schema errors for agents
that are useful for agents to fix issues. With MCP I think they would be fine."

**Revision 2 narrows revision 1 by owner decision.** Unifying every failure shape into one type, replacing MCP
argument decoding, and migrating every other surface are **out of scope**.

## Evidence (2026-09-30)

| Call | Returned | Gap |
| --- | --- | --- |
| MCP `delivery_show` with a malformed id | "automation identity must be a canonical lowercase RFC UUIDv7" | jargon; doesn't name `deliveryId`; no example |
| MCP `delivery_show` with a DM id | "Wake-up was not found … verify the service and wakeupId" | names the wrong resource and field (bug) |
| `message send` to a Claude session claimed by two terminals | `rejected` · "multiple live Claude Code registry records claim this session" | no pids or terminal names, so the sender can't act |
| CLI `message send`, rejected | pretty JSON receipt in human mode | not a short explanation |
| CLI `conversation prompt` without `--to` | `invalidField` · field `--to` · constraint · `correctRequest` | good: the target shape |

Full shape map (context only): `tmp/design-workflows/2026-09-30-agent-error-contract/error-shape-map.md`.

## Authorized needs

| ID | Need | Priority |
| --- | --- | --- |
| U1 | A failure an agent receives has a short explanation (what failed and why) and a next step. | must |
| U2 | An input/schema mistake names the parameter and the expected form, with an example where the form isn't obvious. MCP's existing schema and decode errors are acceptable as they are; only misleading messages are fixed. | must |
| U3 | A message that was not delivered says why in terms the sender can act on (ambiguous Claude session: the pids and terminal names; held: that it's held, its link, and that it will be delivered). | must |
| U4 | The CLI prints failures as one short human line; JSON only with `--json`. | should |

## Delivery

- #112 (messaging): U1–U4 for its own surfaces (send, reply, show, inbox, history, subscribe/wait), the
  ambiguous-session reason, the held result, and the CLI human line for send.
- A small follow-up (can ride the next PR that touches them): the `delivery_show` wrong-resource message and the
  "automation identity" wording.

## Limits

- No new error type across all surfaces; no change to MCP argument decoding; no migration of untouched surfaces.
- Error text never contains secrets or raw provider/storage text; raw text goes to OTel logs (owner decision
  2026-09-30), and the error carries the provider's typed code where one exists.
