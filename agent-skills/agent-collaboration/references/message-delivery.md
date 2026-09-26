# Message delivery

This covers sending to a session with `message send`, `wake send` or a schedule, what the receipt proves, and what to do with large content. The caller decides whom to message and why.

Return: the route (`reachability`), the observed outcome, what it proves, and whether the content went inline or as a file path.

## Read the receipt

Router picks the route from the target. Read `reachability` and `outcome` after every send; the effect can differ from the requested `--delivery` mode.

| `reachability` | Target | Success proves | Completion is observed through |
|---|---|---|---|
| `codexAppServer` | Codex session | turn started, steered, queued, or `startedOrSteered` (one of the two) | the turn, `conversation operation`, or the reply |
| `providerAcp` | Claude or Cursor session Router runs | prompt sent, steered into the running turn, or queued in Router | `conversation operation show\|wait` for the operation ID |
| `claudeCodePeer` | live Claude Code session Router didn't start | the bytes reached its inbox socket | nothing; a reply arrives as a separate message |

`notSubmitted`, `rejected` and `unknown` don't establish delivery. Act on a rejection's reason, next action, code and detail. After `unknown`, inspect with `delivery show` or `conversation operation show` before any resend.

## Choose the mode

| Reachability | `auto` | `queue` | `steer` |
| --- | --- | --- | --- |
| Codex | steers an active turn or starts one | native queue | native steer |
| Claude (Router) | steers a running turn or starts when idle | queues; starts at once when idle | steers a running turn, otherwise `notSubmitted` |
| Cursor (Router) | starts when idle or queues while busy | queues; starts at once when idle | `rejected`: unsupported |
| Live Claude Code | `peerMessageWritten` | `rejected`: unsupported | `peerMessageWritten`, the same as `auto` |

A message queued in Router for Claude or Cursor is lost if the Host restarts before it runs.

## Live Claude Code sessions

Use the exact `claude-local` SessionRef. The message arrives with its origin and a request to reply through Router's `message_send`.
- After the write, the receiver's `crossSessionInbound` setting decides what happens (`accept` delivers, `hold` keeps the message, `refuse` drops it). It may also drop repeats and bursts. Router never learns which, so `peerMessageWritten` is the strongest evidence you get.
- Send one complete message, then wait for the reply. Don't resend because no reply came.
- It reaches only sessions on the same machine.

## Wakes and schedules

| Target | Destinations | A run ends with |
|---|---|---|
| Codex | existing, fresh, or fork | the native turn; fresh-each-run produces a summary |
| Claude or Cursor (Router) | existing sessions only; create the session first | the provider's result; no summary |
| Live Claude Code | existing live sessions only | `peerMessageWritten`, with no completion evidence; the run can't be stopped |

A Claude or Cursor session that never ran a turn can't be loaded after its provider restarts. Delivery is rejected once with `providerSessionNotFound`; create a new conversation instead of retrying.

## Large content

Board messages cap at 64 KiB; messages, wakes and instructions cap at 1 MiB. Message and board text reject control characters other than newline and tab, so pasted terminal output with colour escapes fails.

For logs, diffs, reports and evidence, write a file and send a short summary plus its absolute path:
- `<router-root>/scratch/<root-id>/` for sessions sharing a board root through `--root-message-id`. Only Codex conversations created with that root get write access there; other receivers read it under their own permissions.
- Otherwise the repository's `tmp/` or `docs/wip/`. Don't use system temp for anything that must survive a reboot.

The receiver opens the path with its own tools; nothing is attached, and an `@` in a message is plain text. Paths work only on the same machine.
