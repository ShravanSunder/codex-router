# Message delivery

This reference covers sending to a session with `message send`, `wake send`, or a schedule, reading what the receipt proves for each route, and sending content too large to inline. The caller decides whom to message and why.

Return: the route (`reachability`), the observed outcome, what that outcome does and does not prove, and whether the content went inline or as a file path.

## Routes and receipts

Router picks the route from the target. The receipt's `reachability` names the route, and its `outcome` names the effect. Read both after every send, because the effect can differ from the requested `--delivery` mode.

| `reachability` | Target | A success outcome proves | Observe completion through |
|---|---|---|---|
| `codexAppServer` | Codex session | the native turn started, was steered, was queued, or (`startedOrSteered`) one of started or steered without saying which | the turn, `conversation operation`, or the reply |
| `providerAcp` | Claude or Cursor session that Router runs | the prompt was sent, steered into a running turn, or queued in Router | `conversation operation show\|wait` for the operation ID |
| `claudeCodePeer` | live Claude Code session that Router did not start | the bytes reached that session's inbox socket | nothing; a reply arrives as a separate message |

`notSubmitted`, `rejected`, and `unknown` do not establish delivery. A rejection carries a reason, next action, client code, and detail; act on those fields. After `unknown`, inspect with `delivery show` or `conversation operation show` before any resend.

## Delivery mode by reachability

| Reachability | `auto` | `queue` | `steer` |
| --- | --- | --- | --- |
| Codex app-server | Steer an active turn or start one, as native evidence allows | Native queue behavior | Native steer behavior |
| Router-managed Claude ACP | Steer a running turn or start when idle | Queue, starting at once when idle | Steer a running turn; otherwise `notSubmitted` |
| Router-managed Cursor ACP | Start when idle or queue while busy | Queue, starting at once when idle | `rejected`: unsupported |
| Live Claude Code peer | `peerMessageWritten`; the session may still hold or drop it | `rejected`: unsupported | `peerMessageWritten` in any live status, the same as `auto` |

A message queued in Router for a Claude or Cursor session is lost if the Host restarts before it runs.

## Live Claude Code sessions

Use the exact `claude-local` SessionRef. The message arrives with its origin and a line asking that Claude to reply through Router's `message_send` as itself. It is read between tool calls during a turn, or starts a turn when the session is idle.

The receiving session decides what happens to the message after the write, and Router never learns the result:

- Its `crossSessionInbound` setting (`accept`, `hold`, or `refuse`) applies. When unset, a session that prompts for permissions (`default`, `auto`, `acceptEdits`, `dontAsk`) delivers the message, and a session in `bypassPermissions` holds it behind an approval dialog that drops it after about five minutes.
- It throttles each sender: identical repeats in a short window are dropped, a rapid burst is refused, and at most 50 accepted messages wait to be read.
- It reaches only sessions on the same machine and the same home directory; containers and WSL are separate.

So `peerMessageWritten` is the strongest evidence available. Send one message with everything the receiver needs, then wait for the reply through the caller's chosen listener or reply target. Do not resend because no reply came. A held or refused message is the receiving owner's setting; report it and let the owner change it.

## Wakes and schedules by route

| Target | Supported destinations | How a run ends |
|---|---|---|
| Codex | existing, fresh, or fork | the native turn settles; a fresh-each-run destination produces a summary |
| Claude or Cursor through Router | existing sessions only; create the session first | the provider operation settles; no summary |
| Live Claude Code session | existing live sessions only | `peerMessageWritten`, with no completion evidence; the run cannot be stopped |

A Claude or Cursor session that never ran a turn cannot be loaded after its provider restarts. A delivery to it is rejected once with `providerSessionNotFound`; create a new conversation instead of retrying.

## Large content

Size caps:

| Text | Cap | Stored |
|---|---|---|
| board message | 64 KiB | yes, permanently |
| direct message, wake text, instruction text | 1 MiB | wake and instruction text, yes |
| live Claude Code peer message | just under 1 MiB | no |

Message and board text reject control characters other than newline and tab, so pasted terminal output with colour escapes or carriage returns fails; strip them or send a file.

Keep messages short. Write logs, diffs, reports, and evidence to a file, and send a summary plus the absolute path:

- `<router-root>/scratch/<root-id>/` when the sessions share a board root through `--root-message-id`. Router grants write access there only to Codex conversations created with that root; Claude, Cursor and live Claude Code receivers read it under their own permissions;
- the repository's `tmp/` or `docs/wip/` when the content belongs to the work;
- not system temp for anything that must survive a reboot.

The receiver opens the path with its own tools and permissions; nothing is attached, and an `@` mention inside a message is plain text. A path works only for a receiver on the same machine. `--text-file` reads the message text from a file; the text still counts against the cap.
