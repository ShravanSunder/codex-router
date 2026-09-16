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

### 5a.1 Board reachability from inside the sandbox **(owner, 2026-09-16; verified twice from openai/codex 7f01a84 by Fable and Astra, activities 184 and 191)**

A Router-created thread must run `agent-collaboration board …` from its sandboxed shell. Codex denies Unix sockets unless allowlisted, and the allowlist reaches the seatbelt policy only through Codex's managed network proxy. The verified mechanism and the contract Router implements:

**Request shape.** Router translates `--access` into a Router-owned custom permission profile and sends it per thread; it does not send the native `sandbox` field (Codex rejects `sandbox` together with `permissions`, `thread_processor.rs:1168-1172`, `core/src/config/mod.rs:3271-3277`). The client connection initializes with `experimentalApi` enabled because `thread/start.permissions` is experimental (`initialize_processor.rs:75-77`). The config map is applied through `load_with_overrides` without writing `config.toml` (`config_manager.rs:253-265, 283-320`).

```json
{ "permissions": "router-sidekick-read",
  "config": {
    "features.network_proxy": true,
    "permissions.router-sidekick-read": {
      "extends": ":read-only",
      "network": { "enabled": true, "mode": "full", "domains": {},
                   "unix_sockets": { "<service directory>/control.sock": "allow" },
                   "allow_local_binding": false, "dangerously_allow_all_unix_sockets": false,
                   "enable_socks5": false, "allow_upstream_proxy": false } } } }
```

`router-sidekick-write` is the same block with `"extends": ":workspace"` and the requested workspace roots preserved. `read-only` is the custom profile, not the built-in: the built-in `:read-only` fixes network to restricted (`protocol/src/models.rs:482-489`); extending it contributes only the filesystem and lets the child enable network (`core/src/config/permissions.rs:204-245`). The socket path is the canonical socket file, because the emitted rule is `subpath` and also permits bind (`seatbelt.rs:271-302`).

**Why these values.** The proxy is enabled only when `features.network_proxy` is true and the profile's network policy is enabled (`core/src/config/mod.rs:4508-4563`, `network_config.rs:112-153`). With the proxy on, host decisions deny whenever the allowlist is absent or unmatched, regardless of mode (`network-proxy/src/runtime.rs:644-717`; `http_proxy.rs:240-249, 770-778`), so an empty `domains` map denies all HTTP and HTTPS egress. `mode: "full"` is chosen over `limited` because `limited` materializes MITM and injects a managed CA bundle (`permissions_toml.rs:518-524`, `proxy.rs:853-872`) that a Sidekick has no use for; host policy is identical. Global wildcard denials are rejected (`policy.rs:239-258`), so the deny is expressed by allowing nothing, never by `"*" = "deny"`.

**The boundary as stated to skills.** Allowed: Router's Control socket, the proxy's own loopback listener ports (`seatbelt.rs:321-355`), and platform facilities the filesystem profile carries (syslog on minimal profiles). Denied: every other Unix socket, every HTTP and HTTPS host, local and private addresses (`allow_local_binding: false`). Not claimed: that no communication of any kind is possible, or that separately approved unsandboxed commands are bounded by this. Egress a Sidekick legitimately needs (a package registry during a build) is an approval request under 5a.2, decided per operation by the guardian or the approver, never a static allowlist in the profile.

**Side effects the skill must name.** The proxy sets `HTTP_PROXY`, `HTTPS_PROXY`, package-manager and websocket proxy variables and an empty `NO_PROXY` in the Sidekick's environment (`proxy.rs:768-852`). Credential brokerage stays off (`config.rs:160-181`). Saved exec-policy network rules and managed requirements can overlay the profile (`session/mod.rs:1196-1212`, `network_proxy_spec.rs:327-333, 518-538`), which is why Router verifies the effective policy rather than trusting the request.

**Readback and refusal.** After start or fork Router reads the thread's effective profile and reports `effectiveAccess` as `{ profile, filesystem: read-only|workspace-write, network: { proxyActive, allowedUnixSockets, allowedDomains } }`. Router refuses the dispatch when `proxyActive` is false, `allowedUnixSockets` is not exactly the Control socket, or `allowedDomains` is non-empty, with both requested and effective values, like a model mismatch. `session inspect` reports the same object.

**Proof.** Native tests assert the exact `permissions` and `config` payload on start and fork and the absence of `sandbox`; readback tests cover each refusal. Live macOS proof under both profiles, no interactive grant: `board thread join`, `message post`, `thread listen --once` succeed from the Sidekick's shell; `curl https://example.com` is blocked with `NotAllowed`; a connect to any other Unix socket is denied; the environment shows the proxy variables. Linux (bubblewrap network namespace) is a stated boundary; no claim is made for it.

### 5a.2 Approvals: auto review, then the approver **(owner, 2026-09-16; Astra second opinion folded in)**

Owner constraints: no full or unsandboxed access for Sidekicks; automatic review first; escalation to the coordinator; never a wait on a grant nobody can give.

- Router-created threads send `approvalPolicy: "on-request"` and `approvalsReviewer: "auto_review"` instead of `never`. Verified in openai/codex 7f01a84: `ApprovalsReviewer { User, AutoReview }` (`protocol/src/config_types.rs`); an approved command runs escalated outside the sandbox for that command and `ApprovedForSession` is cached (`core/src/tools/sandboxing.rs`). The dispatch result and `session inspect` report `effectiveApprovalPolicy` and `effectiveApprovalsReviewer` from the thread read; a mismatch is refused like a model mismatch. Requesting is not proof.
- Chain semantics, from `core/src/tools/approvals.rs:435-460, 541-557, 625-650`: the guardian's decision is used directly; only a guardian result of `None` reaches the client (`request_user_approval`); `Denied` and `TimedOut` become errors returned to the model, and a guardian task failure is a denial. Router therefore handles only the requests that reach it as the client. Router never re-triages a guardian denial, never switches reviewer mode, and never retries with another authority. The implementer must establish from source which cases produce `None` under the effective configuration before the CLI promises approver decisions for them.
- Approver routing: Router records `created_by` and `approver` (default the creator; `--approver <SessionRef>` at `--new`/`--fork`) for every thread it creates. A client-exposed request for thread T is delivered only to `approver(T)` through the session delivery layer, carrying: requesting session, operation, cwd and resource scope, the model's justification, effective policy, the offered decision choices exactly as the native request lists them, request id, and expiry. Delivery uses Router's native path, never the Sidekick's board access, so a blocked implementer cannot deadlock its own request.
- Deciding: `approval decide --request-id <R> (--allow | --allow-for-session | --deny) --actor <identity>`. Router accepts a decision only from `approver(T)`'s identity, binds it to the native request id and connection generation as `permission_translation.rs` already does, consumes the pending request on resolution, and maps only offered options. A decision from any other identity, from the requesting thread itself, from an expired, cancelled, duplicate, or old-generation request, is refused with its reason. `--allow-for-session` states in the receipt which future operations it covers, as the native option defines. A decision receipt is not proof the command succeeded; Router makes no claim about the outcome.
- Authority: the approver decides only within authority already delegated by the owner and allowed by the host; the board orchestrator seat is routing metadata, not permission. Anything beyond that is routed to the owner with the exact operation and reason, and the request is denied at expiry if unanswered.
- States, each a distinct closed value in results and in `approval list` (amended 2026-09-16 after the implementer's break report, activity 225): `pendingClientDecision`, `decided` (with the decision), `timedOut`, `approverUnreachable`, `cancelled`. Router never observes a guardian denial (it is final inside Codex and returns to the model as an error), so there is no `guardianDenied` state; the skill tells the implementer that a denied escalation shows up as a failed command in its own turn. Router keeps the request and decision history in its own journal; it does not post outcomes to the work thread and needs no board address at creation. The approver's session may post the outcome on the thread as part of its checkpointing. Router does not correlate a decision with the later tool result; the decision receipt is the record of the decision only, and the command's outcome is visible in the thread's own items. Unreachable approvers (a Claude session today) fail promptly with `approverUnreachable`; nothing queues forever.
- Board socket **(owner)**: Router's Control socket is always allowed by the profile in 5a.1; it is never the subject of an approval request, and the guardian is for other actions (launching an app, reaching a port, writing outside the worktree). If the 5a.1 proof fails, the fallback is the separate MCP design, not approvals.
- Proof: native tests assert the two fields on start and fork and the refusal on mismatch; decision authorization tests for every refused case above; state transitions for the five states; a live demonstration under each `--access` value: launch, join, post, listen, one permitted escalation, one refusal, and return to the same session. The live demonstration is a post-review release gate like the delivery smoke.
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

## 8. Boundaries

- No change to the proxy, board, message delivery, or Participants.
- No fast or priority tier surface, now or as a config passthrough **(owner)**.
- No automatic restart of stale threads; no idle-time rule on either side.
- No merge, install, or restart. Stacked PR after Participants, dated changelog entry.
