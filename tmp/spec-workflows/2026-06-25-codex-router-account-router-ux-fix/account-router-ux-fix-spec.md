# codex-router Account Router UX Fix Spec

Date: 2026-06-25

Status: draft for one `spec-review-swarm` cycle

Source goal: `tmp/workflow-state/2026-06-25-codex-router-account-router-ux-fix/details.md`

## Product Intent

`codex-router` is an account router for Codex traffic.

It owns:

- local router auth, when enabled
- supported-route classification
- upstream account selection
- upstream OAuth credential injection
- quota/state reads needed for selection
- bounded affinity metadata needed to keep Codex continuations on the same upstream account
- redacted routing/status observability

Codex owns everything else:

- prompts, messages, tool calls, files, images, memory, compaction, and model payloads
- retry, fallback, reconnect, and WebSocket lifecycle behavior
- protocol metadata not needed for account routing
- request/response payload validation and interpretation

The router must feel like an installed local app. Normal use should not require callers to know internal state database paths, secret roots, proof flags, or test import commands.

## Requirements

R1. WebSocket pass-through law

The release `serve` path must not enforce router-owned first-frame payload size or shape policy. In particular, release WebSocket routing must not expose `FirstFrameTooLarge`, whole-frame malformed JSON rejection, or a whole-frame request-schema gate.

R2. Bounded metadata only

Before upstream WebSocket open, the router may inspect only bounded account-routing metadata:

- route path is `/v1/responses`
- local-auth carriers needed for local auth and smuggling rejection
- previous-response affinity metadata, when present and bounded
- minimal route/account facts explicitly required by selection

The router must not require prompt-bearing fields, `input`, messages, tool calls, metadata blobs, model payload shape, or arbitrary provider/Codex JSON validity for pass-through.

R3. Exact frame forwarding

After extracting allowed metadata, the exact original first client data frame is forwarded unchanged to the selected upstream. Later frames are also forwarded unchanged. Metadata observation must not mutate or gate forwarding.

R4. Protocol ownership

Production protocol mechanics are owned by the async stack:

- Hyper and `hyper-tungstenite` own local HTTP/WebSocket upgrade plumbing.
- `tokio-tungstenite` owns upstream WebSocket handshakes and frame stream/sink behavior.
- Tokio owns task scheduling and cancellation.

Router code owns account selection and auth/header transformation only. No production release path may reintroduce a hand-rolled HTTP parser, local WebSocket handshake, blocking WebSocket tunnel, application frame truncation, synthetic warmup, synthetic heartbeat, router retry, router fallback, or provider-event-aware close policy.

R5. Response observation is non-gating

Provider/upstream frames and HTTP/SSE chunks are forwarded before non-routing observation work. Affinity/error/status observation must be bounded and non-gating; a slow observer, database write, audit sink, or history recorder must not delay frame/body delivery or close handling.

R6. Account selection and active load are explicit

The spec defines active load as in-flight turn load, not socket lifetime load.

Rationale: Codex creates a turn-scoped model client session and may reuse a physical WebSocket within a turn; it must not be switched mid-turn. A completed turn may leave the socket open, but that open socket alone should not count as active quota burn. If a later request-like frame begins a new turn on the same WebSocket, the router must create a new active reservation for that turn and release it on the next terminal response or close.

This means:

- first routed request frame reserves active load for the selected account
- `response.completed` releases that in-flight reservation without waiting for affinity persistence
- a later request-like frame on the same still-open WebSocket re-reserves active load for the pinned account
- WebSocket account selection remains connection-scoped; re-reserving load is not account switching
- status output must distinguish static persisted quota prediction from live active-turn pressure

Pass-through-safe active-turn state machine:

```text
state idle_before_first_request
  on first data frame:
    select account, reserve active turn, pin account, forward frame unchanged
    -> active_turn
  on control frame:
    handle as WebSocket control, no reservation, no upstream open
  on close:
    close cleanly, no reservation

state active_turn
  on upstream terminal response metadata (`response.completed` or equivalent
  recognized terminal envelope inside bounded observation):
    release active turn before affinity persistence or other slow side effects
    -> idle_after_completion
  on close/error/shutdown:
    release active turn
    -> closed
  on any local data/control frame:
    forward unchanged; do not reselect account

state idle_after_completion
  on local data frame that has bounded top-level request-start evidence:
    reserve a new active turn for the already pinned account
    forward frame unchanged
    -> active_turn
  on local data frame without bounded request-start evidence:
    forward unchanged; do not reserve and do not close
  on control frame:
    handle as WebSocket control
  on close:
    close cleanly
```

Request-start evidence is deliberately narrow and non-gating: a bounded
top-level key scan may identify an enveloped `type == "response.create"` or a
direct Responses-style request marker recognized by Codex-router's existing
selection metadata extractor. Unknown, malformed, future, or fragmented payloads
remain pass-through and must not be closed only because the router cannot
classify them as a new turn. Re-reservation never changes the pinned account and
never mutates the frame.

R7. Status wording matches data source

`quota status` may compute a persisted quota prediction from SQLite, but it must not label that as live router selection when live active reservations or holds are not included. Human output should use wording such as `preferred by quota`, `available by quota`, or `held by quota` unless live router diagnostics are included.

If live diagnostics are included, they must say which inputs are live:

- active turn count per account
- held account, if any
- selected account/reason for a real request, if the command actually evaluated one

R8. SQLite schema is hard-cutover and reproducible

Current source owns the schema. The same `user_version` must not represent different table sets. Any persisted active-client or active-turn table must have a new schema version, migration, repository API, and tests. If active-turn load remains process-local only, stale drift tables must not affect current behavior.

All new or extended runtime SQLite work uses SQLx. Do not add new rusqlite query paths.

R9. CLI command contract is clap-owned

Normal CLI parsing and help are owned by `clap`. Hand parsing may remain only as temporary internal code until touched command surfaces are migrated; this goal touches the normal command surface, so the touched commands must move to typed clap commands.

Normal help must expose only user-facing commands and flags. Proof/test/internal commands must be hidden or moved behind explicit hidden/advanced subcommands.

R10. Installed app defaults

Normal commands default to:

- router root: `~/.codex-router`
- state database: `~/.codex-router/state.sqlite`
- secret root: `~/.codex-router/secrets`
- upstream base URL: the default ChatGPT backend
- profile port: `8787`

Normal user commands must not require `--router-root`, `--state-db`, `--secret-root`, or `--upstream-base-url`.

R11. Account UX

`account login --label <name>` defaults to Codex device auth. It may accept advanced import/auth-json flags only when explicitly documented as advanced or hidden.

`account import-codex-auth` is not part of normal help. It may remain as a hidden/testing command only if implementation needs it.

`account list` renders a `comfy-table` human table by default and JSON only via explicit format flag. Human output prioritizes label, status, credential state, and quota freshness; raw account ids are hidden unless JSON/debug output is requested.

Credential storage policy:

- Normal login stores router-owned account credentials in the configured router secret backend under `~/.codex-router`.
- Secure OS keychain storage is the desired normal backend for macOS. If the current implementation still only has file-backed secrets, normal device login must not silently weaken security: it must either use the secure backend or clearly fail with an actionable message that says secure storage is not implemented yet.
- Plaintext file-backed secret storage is advanced/test-only and requires an explicit hidden/advanced consent flag. It must not be the default path for `account login --label <name>`.
- Migration/import from Codex `auth.json` is hidden/advanced and must not be needed for the normal login flow.
- This goal may implement the secure backend or, if the plan sizes that separately, keep plaintext import hidden while making the normal command fail clearly. It may not present plaintext file storage as the installed-app success path.

R12. Quota UX

The user-facing command is `codex-router quota`.

Default behavior:

1. Render cached persisted quota immediately.
2. Start refresh with an `indicatif` spinner when attached to an interactive terminal.
3. Show refresh results.
4. Re-render updated quota.

`codex-router quota status` may remain as an alias during implementation only if hidden or documented as compatibility. Normal help should teach `codex-router quota`.

Human quota output uses `comfy-table`, Unicode bars, stable concise labels, and no raw account id by default. It shows:

- account
- status
- 5h quota
- weekly quota
- updated/freshness
- active turns or clients when live data is available
- reset credits
- routing/status explanation
- next-use wording

R13. Sessions UX

`codex-router sessions` defaults to current cwd, matching the user's expectation that launching from a folder shows sessions for that folder.

Root filters are explicit flags, not a generic `--scope` vocabulary:

- default / `--cwd`: exact current directory
- `--checkout`: current git checkout/worktree root
- `--repo`: all checkouts/worktrees for the same repository, when discoverable
- `--any`: all sessions

Provider filtering remains independent and defaults to any provider. Source defaults to interactive sessions. The interactive picker must show a two-line human item:

```text
<title or first prompt>
<age>  <branch>  <provider>  <short id>  <cwd/check-out hint>
```

Session DB reads must use SQLx read-only with no create and no write lock.

R14. Proof is current-head proof

Historical receipts do not prove the goal. Every required proof row must be run at the post-fix HEAD, or the proof artifact must include a freshness guard showing that touched source paths did not change.

## Boundary / Separability Map

```text
Codex CLI
  owns: prompts, turns, retries, fallback, WS lifecycle
  sends: HTTP/SSE and WS traffic

        pass-through except local auth/account routing
        ───────────────────────────────────────────────►

codex-router proxy
  owns: local auth, route support, account selection, credential injection,
        bounded affinity metadata, redacted diagnostics
  must not own: Codex payload validation, provider protocol semantics,
        retry/fallback, message truncation, synthetic application behavior

        typed async state/auth interfaces
        ───────────────────────────────►

codex-router-state / secret store
  owns: schema, migrations, account records, quota snapshots, affinity owners,
        credential generation metadata, secret material
  exposes: SQLx-backed async runtime APIs and CLI read APIs

        OAuth credentials
        ─────────────────►

Upstream ChatGPT backend
  owns: request/response schema validation, quota responses, model responses,
        provider errors, Codex protocol metadata
```

## WebSocket Contract

The local WebSocket route is `/v1/responses`.

Sequence:

```text
local HTTP upgrade request
  -> reject unsupported route or invalid local auth before local accept
  -> accept local WebSocket via Hyper / hyper-tungstenite
  -> wait for first data frame or clean client close
  -> extract bounded metadata needed for account routing
  -> select account
  -> resolve credential
  -> open upstream WebSocket with tokio-tungstenite
  -> forward exact first frame unchanged
  -> forward both directions until close/error/shutdown
```

Allowed pre-upstream failure cases:

- unsupported route
- local auth failure
- forbidden top-level auth smuggling carrier
- missing required account-routing metadata when routing truly cannot proceed
- affinity owner failure when previous-response affinity is explicitly present
- account selection failure
- credential resolution failure
- upstream open failure

Forbidden pre-upstream failure cases:

- arbitrary payload too large for an invented router cap
- malformed whole-frame JSON when the frame could still be a Codex-owned payload
- unknown future Codex shape
- prompt/tool/message content containing strings such as `previous_response_id`
- whole-frame schema mismatch

## CLI Surface

Normal help should be roughly:

```text
codex-router

Commands:
  serve             Run the local router
  account login     Add an OAuth account
  account list      Show router accounts
  quota             Show quota, refresh, and routing status
  sessions          Pick or resume a Codex session
  doctor            Diagnose local setup
  profile print     Print Codex profile snippet
```

Hidden or advanced:

- token commands, unless local token mode is explicitly reintroduced for advanced users
- profile write, unless gated and documented as a home-write operation
- import-codex-auth
- live quota diagnostics
- serve proof flags such as max connections, audit file, and registry report
- now/test clocks

The plan may keep hidden compatibility aliases if removing them would slow the fix, but normal help must not teach them.

## Quota And Routing Status Contract

`quota` output answers: "Which account should I use now, based on known quota and live router pressure when available?"

If the command only uses SQLite persisted data, it must say so through labels or notes:

- `preferred by quota`
- `available by quota`
- `blocked by quota`
- `cached <age>`
- `refreshing...`
- `updated <age>`

If live router pressure is not available because the CLI is not connected to a running server, the output must not invent client counts. It may show `active turns: not connected` or omit the column.

If a live server report is available, the column is `active turns`, not vague `clients`, unless it truly counts socket/client lifetime.

## Data / Schema Contract

Schema versioning is exact:

- one `user_version` means one expected base table set
- optional history tables must be included in the version contract or clearly separated as additive extension tables
- drifted tables from old worktrees must be ignored or migrated intentionally

For this goal, the spec preference is process-local active-turn tracking for live selection, plus optional read-only live diagnostics from the running server. Persisting active turns is not required unless the implementation plan proves it is needed.

## Security Context

Sensitive assets:

- OAuth access/refresh tokens
- upstream Authorization headers
- router local-auth tokens if advanced mode exists
- account ids and unsafe labels
- affinity hash secret
- prompts, tool args, request/response bodies
- Codex session ids and cwd paths

Required invariants:

- no OAuth or router token in logs, traces, tables, smoke transcripts, or test artifacts
- no raw request/response body captured in shared proof artifacts
- no raw full WebSocket first frame captured in logs/traces
- local-auth smuggling check remains top-level, bounded, and key-only
- session DB opens read-only and does not create or lock Codex's state DB

Top-level auth-smuggling detector contract:

- The detector scans only the bounded prefix or bounded top-level key stream
  needed to decide whether a forbidden direct top-level auth carrier key exists.
- It must not require the whole payload to parse as JSON.
- It must not validate request schema.
- It must not inspect nested object keys, prompt text, tool arguments, metadata
  values, or arbitrary strings for token-like words.
- A malformed, future-shaped, fragmented, or very large Codex payload forwards
  unless a definite forbidden top-level auth carrier key is found inside the
  explicit scan bound.
- Definite forbidden carrier examples are direct top-level local/upstream auth
  carrier keys that would conflict with router-owned auth/header handling.

## Proof Expectations

The implementation plan must map these to exact commands:

- unit red/green: first-frame router no longer rejects >1 MiB legal frame
- unit red/green: malformed/future/nested payloads do not fail because of whole-frame validation
- unit red/green: top-level auth smuggling still rejects
- unit red/green: nested/prompt/tool token-like strings and nested auth-looking
  keys do not trigger auth-smuggling rejection
- integration: real serve path forwards >1 MiB first WS frame byte-identically
- integration: same-socket second-turn request-like frame re-reserves active
  turn load for the pinned account, while unknown post-completion frames remain
  pass-through without reservation or close
- integration: slow metadata observer cannot delay WS frame forwarding
- integration: same WebSocket re-reserves active turn load for a later request-like frame after completion
- integration: `quota` distinguishes cached/static prediction from live diagnostics
- migration: empty DB and drifted v7 DB both produce supported current schema behavior
- CLI tests: normal help hides internal/proof/live/import noise
- CLI tests: `account login --label <name>` defaults to device auth
- CLI tests: `account list` uses table/json formats correctly
- CLI tests: `sessions` default is cwd, with `--checkout`, `--repo`, and `--any`
- smoke: installed `codex-router serve` plus installed Codex uses WebSocket without `CODEX_ROUTER_TOKEN`
- e2e/soak: three concurrent installed Codex clients prove redacted selected-account tags and WebSocket continuity at current HEAD
- structural: release path forbids `FirstFrameTooLarge`, release `FirstFramePolicy`, whole-frame first-frame JSON gate, blocking WS tunnel, and hidden production parser/handshake ownership
- quality: fmt, clippy with repo lints, relevant cargo tests, proof-matrix rows, and implementation review

## Open Decisions

OD1. Should `live quota` diagnostics be removed from the binary entirely or hidden under an advanced command?

Recommended answer: hide advanced diagnostics for now; remove only if they create maintenance drag during clap migration.

OD2. Should active-turn diagnostics require a running router control endpoint?

Recommended answer: use existing/nearby runtime report machinery or a simple local report hook only if needed for proof; do not invent a full admin API in this goal.

OD3. Should `token` commands remain hidden?

Recommended answer: yes. Tokenless is normal mode; local token mode is advanced.

## Next Workflow

Run exactly one `shravan-dev-workflow:spec-review-swarm` cycle against this spec. Address accepted findings in this spec, then move to `shravan-dev-workflow:plan-creation-swarm`.

phase_result: complete
evidence: this spec file plus goal details and current code anchors
recommended_next_workflow: shravan-dev-workflow:spec-review-swarm
recommended_transition_reason: The expanded account-router/CLI contract is now explicit enough for one adversarial spec review pass.
