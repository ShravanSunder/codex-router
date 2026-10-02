# Agent error messages — Program Design (revision 2)

Realizes [specification.md](specification.md) R1–R5 (revision 2, narrowed by owner decision 2026-09-30).
Revision 1's single `AgentFailure` type, MCP raw-argument decoder and all-surface migration are withdrawn.

## Components

| Component | Change |
| --- | --- |
| `claude-code-peer-messaging` · registry | `PeerSessionLookup::Ambiguous { claims: Vec<PeerClaim { pid, name, cwd }> }` replaces the string reason for duplicate claims (R3) |
| `claude_code_peer_delivery_route.rs` | maps `Ambiguous` to `rejected · liveElsewhere`; `detail` = the one-line explanation (names shortened to fit); the claims go in a structured `claims` field of the rejection |
| push records (push-format B/C) | `last_outcome_json` stores the typed outcome (reason, next step, provider code, correlation id); written with `delivery_state`; `show` returns it (R3) |
| `session_message_dispatch` + CLI `message send` | result `{ push, delivery: delivered \| held \| rejected \| unknown, explanation?, next? }`; exit codes per R4 |
| `agent-collaboration` · `failure_line.rs` | renders `error: <explanation> — <next step>` and the `held:` line for in-scope commands; `--json` unchanged in shape except the added fields |
| CLI argument parsing for in-scope commands | invalid values name the flag and expected form with an example (existing `invalidField` shape: `field`, `constraint`, `nextAction`) |
| producers of provider errors in scope | map raw text to a typed reason + provider code; log the raw text via `tracing::warn!` (OTel logs) with a correlation id (UUIDv7) that the error line shows |
| follow-up: `wakeup_lifecycle_dispatch.rs` (delivery_show not-found), `automation_identity.rs` message | name the right resource/parameter and show an example (R5) |

## Line budget

The explanation line is at most 240 characters, measured after escaping. The next step is rendered first and
reserved; claims are then appended in registry order as `pid <n> <name shortened to 24 chars>` while the line
still fits, and the rest become `+N more`. Pids and full names always stay in the structured `claims`. Next-step commands are real CLI syntax (for
example `agent-collaboration show <link>`, `board thread join --root-message-id <id> …` with its required
flags).

## Proof seams

As the Specification's proof table: real CLI binary tests, real peer route with a two-record registry fixture,
real Codex route + owner for held, `show` read-back, and a synthetic-secret marker.
