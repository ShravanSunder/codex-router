# Agent error messages — Specification (revision 2)

Requirements: [requirements.md](requirements.md) (U1–U4, revision 2: narrowed by owner decision).
[program-design.md](program-design.md) says how.

## Obligations

- **R1 — short and actionable (U1).** Every failure on the in-scope surfaces carries a one-line explanation
  (what failed, why) and a next step (the command to run, the form to use, or what to wait for). The line
  names the caller's parameter or target, uses product words (not internal type names), and contains no raw
  provider/storage text or secrets. Provider errors include the provider's typed code where one exists; the raw
  text goes to OTel logs with a correlation id that the error shows.
- **R2 — input mistakes (U2).** On the in-scope CLI commands, an invalid or missing input names the flag or
  field and the expected form, with an example for ids, JSON and durations. MCP keeps its current schema and
  decode errors.
- **R3 — not delivered (U3).**
  - A Claude session claimed by several live terminals: `rejected`; the line says how many terminals and lists
    claims (pid + name, names shortened) in order while they fit the 240-character line, then `+N more`; the
    structured `claims` field always holds every live claim with its pid and, when the registry record has
    them, the full name and cwd. A claim without a name is shown by its cwd basename, else as `unnamed
    terminal`; an incomplete claim is never dropped from the ambiguity check (dropping it would make the
    other claim look unique and misdeliver). Next step "close one of these terminals, or run `/branch` in one
    of them".
  - A DM whose target is not running: the result is `held` (push-format R14), names the push link, says it
    will be delivered when the session is next running and expires after 30 days; next step
    `agent-collaboration show <link>`. It never suggests resending.
  - Rejected and unknown outcomes are stored on the push record and returned by `show <link>` later.
- **R4 — CLI human line (U4).** Without `--json`, a failure prints one line: `error: <explanation> — <next
  step>`. A held send prints `held: <link> — delivered when <target> is next running`. Exit codes: delivered and
  held 0, rejected 4, unknown 5.
- **R5 — small fixes (follow-up).** `delivery_show` not-found names the delivery id (and, for a push id, points
  to `show <link>`); "automation identity must be…" names the parameter (`deliveryId`, `wakeupId`, …) and shows
  a UUIDv7 example.

## Scenarios

| # | Given | When | Then |
| --- | --- | --- | --- |
| S1 | two live terminals share a Claude session | `message send` | `error: <name> is open in 2 terminals (pid 52304 agent-studio-pane-fixes-b4, pid 68833 ipc-remote-zmx) — close one, or run /branch in one of them`; exit 4 |
| S2 | the target Codex thread is not loaded | `message send` | `held: router://…/push/… — delivered when <target> is next running`; exit 0 |
| S3 | a rejected DM, later | `show <link>` | the record with its stored outcome and next step |
| S4 | CLI `message send --to '{bad json'` | — | `error: --to: expected a SessionRef JSON like {"endpoint":…,"sessionId":"…"} — fix the --to value` |

## Proof

| Obligation | Proof |
| --- | --- |
| R1, R4 | CLI tests on the real binary for each in-scope command's failure paths: exact first line, exit code, no raw text |
| R2 | CLI tests with malformed ids, JSON and durations on the in-scope commands |
| R3 | the real peer route with two live registry records (S1); the real route and owner for an unloaded thread (S2); `show` after rejected/unknown (S3) |
| R1 secrets + OTel link | a synthetic secret marker in an injected provider error never appears in the line or the stored outcome; the captured OTel log record (test tracing subscriber) holds the raw text, and its correlation id and provider code equal the ones in the error and the stored outcome |
| R3 many claims | five claims with long names: line ≤ 240 characters with `+N more`; structured `claims` has all five |
| R5 | MCP `delivery_show` tests for the corrected messages (follow-up) |
