# Thread model choice: explicit model and reasoning effort on every dispatch

Date: 2026-09-15 (amended 2026-09-16: process). Status: owner-accepted design, ready to implement. Design is owned by the owner and the Fable design session; the implementer builds from this document and does not author Specification or Program Design artifacts. Fable is final reviewer of the implementation. First PR of the collaboration CLI DX stack, after Thread Participants. Author: Fable design session on the owner's behalf. Owner decisions are marked **(owner)**.

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
  --new     --model <id> --effort <level> --access <read-only|workspace-write> --text-file <path> --json
  --session <id>         --effort <level> --text-file <path> --json
  --fork <session-id> --model <id> --effort <level> --access <read-only|workspace-write> --text-file <path> --json
```

`--fork` is new: it forks the named thread (native `thread/fork`) with the stated choice and prompts the fork. `--new`, `--session`, `--fork` are mutually exclusive and one is required.

Result additions (all three forms): `effectiveModel`, `effectiveEffort`, `effectiveAccess`, `idleSeconds` (0 for a fresh thread), and the session target.

`sessions list --json` rows gain `model`, `reasoningEffort`, `idleSeconds`.

Schedule definitions that start fresh threads gain required `model` and `effort` fields; a definition without them is refused at `schedule create` and `schedule update` with the field name. Existing stored definitions without the fields fail at `schedule prepare` with the same refusal; there is no migration that invents values.

## 5. Native mapping

| Dispatch | Native call | Fields sent |
|---|---|---|
| new | `thread/start` then `turn/start` | `model`, `config.model_reasoning_effort`, `allowProviderModelFallback: false`, `sandbox`, `approvalPolicy: never` on start; nothing model-related on the turn |
| resume | `thread/resume` (if unloaded) then `turn/start` | no model on resume; `effort` on the turn |
| fork | `thread/fork` then `turn/start` | as new |
| scheduled fresh | `thread/start` | as new |

Verification: after start/resume/fork read the echoed `model`; after the turn completes, `thread/read` and compare `reasoningEffort` to the request. Reuse the summary worker's mismatch handling as the single owner of this check.

## 5a. Access on creation **(owner, 2026-09-16)**

A Router-created thread is unattended: the ACP path cancels permission requests, so a session that needs an approval hangs or fails. The access boundary is therefore stated at creation and enforced by the Codex sandbox, not by prompts. `--new` and `--fork` take a required closed enum `--access read-only | workspace-write`; there is no full-access value and no passthrough to one. Router sends `sandbox` accordingly and `approvalPolicy: "never"` on `thread/start` and `thread/fork` (the summary worker in `collaboration-service/src/summary_native_worker.rs` already uses this pair with `read-only`). A `--session` dispatch never changes access. The dispatch result and `session inspect` report `effectiveAccess` from the thread read; a mismatch is refused like a model mismatch. Skill guidance: implementer Sidekicks get `workspace-write` on their own worktree; reviewers and research Sidekicks get `read-only`; a command that needs escalation fails inside the sandbox and the Sidekick reports it rather than waiting for an approval nobody can give.

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

## 8. Boundaries

- No change to the proxy, board, message delivery, or Participants.
- No fast or priority tier surface, now or as a config passthrough **(owner)**.
- No automatic restart of stale threads; no idle-time rule on either side.
- No merge, install, or restart. Stacked PR after Participants, dated changelog entry.
