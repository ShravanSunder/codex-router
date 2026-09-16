# Thread model choice: explicit model and reasoning effort on every dispatch

Date: 2026-09-15, amended 2026-09-16. Model/effort behavior remains as specified below. Access, scratch, approval inheritance, and settings evidence follow the current owner requirements in [Router access requirements](2026-09-16-router-access-requirements.md). The socket-only integration and cold-resume behavior remain proof obligations, not established capabilities.

## 1. Problem

Router starts, resumes, and forks Codex threads without ever naming a model or reasoning effort, so every thread inherits the host's config silently. A coordinator that must run a Sidekick at `gpt-5.6-sol` / `medium` cannot say so through Router, and cannot see from the dispatch receipt what it actually got. The native protocol already supports both fields on thread start, resume, fork, and turn, and reports both on thread read; Router just never sends them.

## 2. Ground truth this design rests on

- Codex app-server v2: `thread/start`, `thread/resume`, `thread/fork` accept `model` and a `config` override map that carries `model_reasoning_effort`. `turn/start` accepts `model` and `effort`; either changes the thread for that turn and all later turns, and Codex persists the change. `thread/read` reports `model` and `reasoningEffort` (current when loaded, last persisted otherwise). Router already exposes these through `session inspect`.
- Prompt cache: keyed on model plus exact input prefix. A model change invalidates the cache. An effort change does not.
- Router today: `conversation prompt --new|--session` sends neither field; the ACP adapter sends `cwd`, MCP config overrides, and workspace roots; scheduled fresh threads send `cwd` and developer instructions; the internal summary worker is the only caller that pins a model (`allowProviderModelFallback: false`) and verifies the echo.

## 3. Domain model

| Term | Meaning |
|---|---|
| Model | the exact model id a thread runs on; fixed for the thread's life |
| Effort | the reasoning effort level for a turn; closed set as advertised by Codex (`low`, `medium`, `high`, `xhigh`, and any other value the native runtime advertises); may change between turns |
| Model choice | one Model plus one Effort, stated by the caller |
| Dispatch | any Router operation that causes a Codex turn: new thread, resume, fork, scheduled run |
| Effective choice | the Model and Effort the native runtime reports after the dispatch |

Invariants:

1. Every Dispatch states an Effort explicitly **(owner)**. There is no default and no fallback to the thread's persisted value; a missing `--effort` is a validation error naming the flag.
2. Every Dispatch that creates a thread (new, fork, scheduled fresh thread) states a Model explicitly **(owner)**. A missing `--model` is a validation error naming the flag.
3. A Dispatch to an existing thread never carries a Model. `--model` with `--session` is a validation error: "model is fixed for a thread; fork to change it". Router never sends `model` on `turn/start`.
4. Model and Effort are two flags, never a fused syntax **(owner)**. The acpx bracket form is documented as a mapping in the skill reference, not accepted by Router.
5. Router sends the choice with provider fallback disabled and verifies the Effective choice from the native response (thread start/resume/fork echo; `thread/read` after a turn). A mismatch is a refusal that reports both requested and effective values; it is never silently accepted.
6. Every Dispatch result and every session listing reports the Effective choice, so a coordinator verifies from the receipt, not from a second inspect call.
7. Router exposes no fast or priority tier of any kind **(owner)**. No flag, no config passthrough that could reach a service tier setting. This is a boundary, not a default.
8. Thread age is reported, never acted on. Every Dispatch result and session listing carries `idleSeconds` (now minus the thread's `recencyAt`) as cost and observability information only. There is no continue-or-restart rule anywhere **(owner, amended 2026-09-16)**: a Sidekick keeps its session across any idle period, a cold resume is an accepted cost, and no skill schedules keep-alive turns.

## 4. CLI

```text
agent-collaboration conversation prompt --endpoint <id> --cwd <path>
  --new     --model <id> --effort <level> --access <write-restricted|workspace-write> --text-file <path> --json
  --session <id>         --effort <level> --text-file <path> --json
  --fork <session-id> --model <id> --effort <level> --access <write-restricted|workspace-write> --text-file <path> --json
```

`--fork` is new: it forks the named thread (native `thread/fork`) with the stated choice and prompts the fork. `--new`, `--session`, `--fork` are mutually exclusive and one is required.

Result additions (all three forms): `effectiveModel`, `effectiveEffort`, `effectiveAccess`, `idleSeconds` (0 for a fresh thread), and the session target.

`sessions list --json` rows gain `model`, `reasoningEffort`, `idleSeconds`.

Schedule definitions that start fresh threads gain required `model` and `effort` fields; a definition without them is refused at `schedule create` and `schedule update` with the field name. Existing stored definitions without the fields fail at `schedule prepare` with the same refusal; there is no migration that invents values.

## 5. Native mapping

| Dispatch | Native call | Fields sent |
|---|---|---|
| new | `thread/start` then `turn/start` | `model`, `config.model_reasoning_effort`, `allowProviderModelFallback: false`, access configuration from §5a on start; approval settings omitted; nothing model-related on the turn |
| resume | `thread/resume` (if unloaded) then `turn/start` | no model on resume; `effort` on the turn |
| fork | `thread/fork` then `turn/start` | as new |
| scheduled fresh | `thread/start` | as new |

Verification: after start/resume/fork read the echoed `model`; after the turn completes, `thread/read` and compare `reasoningEffort` to the request. Reuse the summary worker's mismatch handling as the single owner of this check.

## 5a. Access and writable work areas

Requirements: A1–A7 in [Router access requirements](2026-09-16-router-access-requirements.md). These sections replace the former fixed approval settings, deny-all network profile, and effective-network readback requirements.

The caller selects one access enum through `--access write-restricted | workspace-write` on new and fork. Router translates that choice into native request arguments/configuration; callers do not construct native permission objects or maintain a permanent profile file for every agent. Native `permissions` selection and legacy `sandbox` selection must not be sent together.

| Access | Writable locations |
| --- | --- |
| `write-restricted` | Shared system scratch, the selected project's `tmp/`, and the selected project's `docs/wip/`; other repository paths remain read-only |
| `workspace-write` | The selected worktree and shared system scratch; existing native temporary-directory behavior is preserved |

`write-restricted` describes repository source access with the listed exceptions. It does not prohibit edits to existing files under `tmp/` or `docs/wip/`. Board participation roles do not grant filesystem access. Approval policy and reviewer are inherited from app-server configuration on creation and native saved-session behavior on resume; Router does not force `never`, `on-request`, or `auto_review`, and does not override them merely because an agent has a particular board role. In the owner's current setup, the inherited values are `on-request` and `auto_review`.

### Shared scratch and session continuity

For work launched with a message-board thread, `--root-message-id <id>` identifies the shared scratch scope. This is the root message ID, not the topic ID, agent session ID, or Codex thread ID. Agents launched for the same root resolve to the same owner-private `<scratch-base>/<root-message-id>/` directory. The selected root and resolved path are exposed in setup evidence and retained across reconnects. For work without a root, Router supplies session-scoped scratch and reports that scope rather than guessing a board thread.

```text
caller: access enum + project + optional board root message ID
  -> Router session
       -> selected repository access
       -> writable project tmp/ and docs/wip/
       -> writable scratch shared by the root message ID
       -> inherited approval behavior
       -> exact Router Control socket exception
```

Agents may use session-named subdirectories for their own files and deliberately share named files. These are naming conventions, not isolation within the shared directory; Router does not promise conflict-free concurrent edits to one file or add a scratch locking service. Scratch is disposable. Decisions and durable results belong in project artifacts or the board. Thread resolution or listener expiry must not automatically delete scratch used by another session.

Resume keeps the selected access, project, root mapping, and scratch path. Fork explicitly selects access and retains a work-root association only when supplied by its caller. Inline permission definitions must be supplied again when the native runtime does not restore them; a saved profile name alone is not proof that its rules survived. Missing reconstruction inputs or an unexpected access/path change must be reported before dispatching more work, rather than silently selecting a default. No automatic replay after an uncertain setup outcome.

### 5a.1 Router socket exception

Router's managed Codex configuration supplies access to the canonical Control socket file for the selected Router service by default. The exception must not grant its containing directory, all Unix sockets, unrestricted filesystem access, or unrelated network access. Existing app-server network and approval behavior otherwise remains intact. Socket communication does not require an approval request.

A writable scratch directory does not itself authorize Unix-socket communication. A socket entry that never reaches native enforcement does not satisfy this contract. Enabling a network proxy with changed domain rules is not an equivalent implementation.

Current evidence: Codex 0.154.0 sandbox execution supports inline filesystem profiles and passed read-only plus scratch allowed/denied checks. At inspected upstream revision `7f01a84`, the direct extra-socket argument is wired in the sandbox debug command, while the ordinary sandbox manager supplies an empty extra-socket list. A supported normal app-server route that preserves the existing policy is still unproven. An upstream Codex change, a production restart, or broader permission changes require an explicit handoff to the orchestrator; they are not authorized fallback repairs.

### Settings evidence and completion

Setup records distinguish requested Router access from settings actually observed in native start, fork, or resume responses. Settings evidence identifies its source and observation time. The stable public settings observation is either `observed` with the available native values and source, or `unavailable` with a reason; absent values must never be populated from the request. Observed fields include native filesystem/sandbox settings, permission-profile identity when supplied, approval policy, and approval reviewer. A profile ID is provenance, not a complete effective network-policy snapshot.

After a completed turn, `thread/read` continues to verify model and effort. Permission fields are not required from that response: their absence must not convert a successfully completed turn into a permission projection failure. Previously observed settings remain labeled with their original source/time; they are not presented as freshly checked after the turn. `session inspect` reports available observations or explicit unavailability, and must not resume a session merely to manufacture a read-only inspection result. Required setup evidence that is missing or incompatible with the requested access remains a setup refusal before work is submitted.

Cold-resume restoration and native command enforcement must be demonstrated through the real app-server path. A debug-sandbox test establishes filesystem enforcement only, not native resume, approval routing, or Router socket integration.

### 5a.2 Inherited approvals and client-exposed requests

Router inherits native approval behavior. Where the app-server selects automatic review, it runs first. The broker below handles only requests actually exposed to Router as the client; it does not impose an approval policy or reopen a native denial.

- Router omits approval-policy and reviewer overrides on setup and resume. Settings evidence follows §5a; no approval-setting fields are expected from `thread/read`. A native approval decision may permit an individual operation beyond the ordinary sandbox boundary; write-restricted access with automatic review is not a promise that escalation can never permit a write.
- Chain semantics, from `core/src/tools/approvals.rs:435-460, 541-557, 625-650`: the guardian's decision is used directly; only a guardian result of `None` reaches the client (`request_user_approval`); `Denied` and `TimedOut` become errors returned to the model, and a guardian task failure is a denial. Router therefore handles only the requests that reach it as the client. Router never re-triages a guardian denial, never switches reviewer mode, and never retries with another authority. The implementer must establish from source which cases produce `None` under the effective configuration before the CLI promises approver decisions for them.
- Approver routing: Router records `created_by` and `approver` (default the creator; `--approver <SessionRef>` at `--new`/`--fork`) for every thread it creates. A client-exposed request for thread T is delivered only to `approver(T)` through the session delivery layer, carrying: requesting session, operation, cwd and resource scope, the model's justification, effective policy, the offered decision choices exactly as the native request lists them, request id, and expiry. Delivery uses Router's native path, never the Sidekick's board access, so a blocked implementer cannot deadlock its own request.
- Deciding: `approval decide --request-id <R> (--allow | --allow-for-session | --deny) --actor <identity>`. Router accepts a decision only from `approver(T)`'s identity, binds it to the native request id and connection generation as `permission_translation.rs` already does, consumes the pending request on resolution, and maps only offered options. A decision from any other identity, from the requesting thread itself, from an expired, cancelled, duplicate, or old-generation request, is refused with its reason. `--allow-for-session` states in the receipt which future operations it covers, as the native option defines. A decision receipt is not proof the command succeeded; Router makes no claim about the outcome.
- Authority: the approver decides only within authority already delegated by the owner and allowed by the host; the board orchestrator seat is routing metadata, not permission. Anything beyond that is routed to the owner with the exact operation and reason, and the request is denied at expiry if unanswered.
- States, each a distinct closed value in results and in `approval list` (amended 2026-09-16 after the implementer's break report, activity 225): `pendingClientDecision`, `decided` (with the decision), `timedOut`, `approverUnreachable`, `cancelled`. Router never observes a guardian denial (it is final inside Codex and returns to the model as an error), so there is no `guardianDenied` state; the skill tells the implementer that a denied escalation shows up as a failed command in its own turn. Router keeps the request and decision history in its own journal; it does not post outcomes to the work thread and needs no board address at creation. The approver's session may post the outcome on the thread as part of its checkpointing. Router does not correlate a decision with the later tool result; the decision receipt is the record of the decision only, and the command's outcome is visible in the thread's own items. Unreachable approvers (a Claude session today) fail promptly with `approverUnreachable`; nothing queues forever.
- Board socket **(owner)**: Router's Control socket is always allowed by the profile in 5a.1; it is never the subject of an approval request, and the guardian is for other actions (launching an app, reaching a port, writing outside the worktree). If §5a.1 cannot be met, report the capability gap; no automatic MCP substitution or per-call socket approval workaround.
- Proof: native payload checks establish that approval overrides are absent. Real app-server execution establishes inherited approval behavior, permitted and refused operations, callback authority where callbacks are exposed, and return to the same session. No fixture may invent settings in `thread/read`. The socket and scratch journeys in §7 are required before PR readiness, as requested by the owner.
- Skills: manage-agents owns assignment authority and escalation responsibility; the Router skill documents the request, decision, and wait mechanics and the states above; `my_agents.md` carries only the escalation pointer.

## 5b. Sidekick sessions are top-level **(owner, 2026-09-16)**

A thread started or forked through Router is a persistent top-level session: it appears under `--source interactive` in listings, carries its own name, model, effort, and status, and is never recorded as a subagent. Subagents are disposable native children; Sidekicks are not. Router applies no keep-alive and no 26-minute window to a Sidekick; a cold resume is an accepted cost, and `idleSeconds` is reported so the caller can see it. Proof adds: a `--new` thread shows `source: interactive` in the scoped listing.

## 6. Skill reference

`agent-collaboration` `references/` gains one short section: `--model` on `--new` and `--fork`, `--effort` on every dispatch, never `--model` with `--session`, the acpx bracket mapping, and that `idleSeconds` is cost information. The Router skill references no `manage-agents` policy. `manage-agents` (ai-tools, separate change) states: a Sidekick is one persistent session; resume it whatever its idle time; state the effort on each dispatch; the model was fixed when it was created.

## 7. Proof

- Validation: each omitted flag and the `--model --session` combination fail naming the flag before any native call.
- Native path tests: new, resume, fork, scheduled send exactly the fields in section 5 and nothing else model-related; a mismatched echo is refused with both values.
- Live proof: start a thread at one choice, inspect shows it; resume with a different effort, inspect shows the new effort and the same model; fork with a different model, inspect shows it on the fork and the original unchanged.
- Result shape: every dispatch result carries the effective choice and `idleSeconds`; `sessions list` rows carry all three.
- Repo checks: fmt, clippy, tests, SQLx if touched, `git diff --check`.

### Access and continuity proof

| Need | Required observable evidence |
| --- | --- |
| A1, A2 | Real native commands can read source and write both project work areas and shared scratch; write-restricted denies other source writes and unrelated scratch, while workspace-write permits selected worktree writes. |
| A3 | New/fork requests inherit native approval settings; native approval behavior is exercised without Router forcing a reviewer or policy. |
| A4 | Two sessions associated with one root see the same shared file; separate roots resolve to separate scratch directories; simultaneous writes to distinct agent-owned files survive. This does not claim exclusion of deliberate access under broader inherited permissions. |
| A5 | After a genuine cold app-server restart, the same session recovers access, project write exceptions, root mapping, and scratch path; allowed and denied operations are repeated. |
| A6 | Normal app-server tool execution can join/post/listen through the exact Router socket without escalation, while unrelated access retains the baseline policy. No debug-only socket option stands in for this proof. |
| A7 | Completed native output produces a successful receipt despite absent permission fields in `thread/read`; setup observations retain source/time, and inspect reports unavailable evidence honestly. |

These proofs are additive to model/effort, approval authorization, listen delivery, and repository gates. Do not repeat unchanged completed suites without a relevant change or unresolved concern.

## 8. Boundaries

- No broader proxy or network-policy change for socket access. Board root identity is consumed for scratch scope; this amendment does not redesign message delivery or participant roles.
- No fast or priority tier surface, now or as a config passthrough **(owner)**.
- No automatic restart of stale threads; no idle-time rule on either side.
- No merge, install, or restart. Stacked PR after Participants, dated changelog entry.
