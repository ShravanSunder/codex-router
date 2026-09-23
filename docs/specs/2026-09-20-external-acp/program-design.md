# PR2 External ACP Program Design

## Components and ownership

```text
CLI / MCP -> collaboration-client -> existing Control socket
                                      -> collaboration-service typed dispatch
                                          -> Host-owned provider supervisor
                                              -> official Rust ACP SDK
                                                  -> Claude bridge / Cursor ACP
```


- `collaboration-protocol` owns provider-neutral identities, requested policy, effective mapping status, operation effects, and uncertainty schemas.
- `codex-router-host` owns the provider-session supervisor, dynamic external endpoint bindings, child-process lifetime, and the metadata-only operation record/reconciliation owner.
- `collaboration-client` owns typed Control calls for external providers and the existing Codex ACP calls. External provider ACP correlation, updates and permission callbacks run under the Host-owned provider binding. Its SDK calls remain call-local and do not own provider processes or durable records.
- Existing CLI and MCP adapters remain presentation layers over the shared client boundary.
- Provider settings, authentication, MCP config files, and provider session data remain provider-owned.
- The existing board/listen path remains the user-facing coordination surface.

Each external provider binding is exposed through an endpoint-scoped `SessionRef` whose opaque session ID resolves through the Host-owned dynamic binding to a provider kind/runtime binding and provider conversation ID. The binding is created only after initialize/capability negotiation and is advertised through the existing service endpoint discovery/control surface while active. The advertisement distinguishes the caller route (existing Control socket and typed conversation methods) from the upstream carrier (`stdioAcp`, Host-private child pipes). It also carries endpoint ID, provider kind, runtime identity, capability/evidence set, availability, service epoch, and binding generation. Callers never open child stdio or launch a provider themselves. Codex's existing `codex-local` endpoint and Unix ACP path remain unchanged; external stdio bindings are a separate endpoint class. The Host removes an advertisement when its binding closes and increments binding generation on replacement. SessionRef remains the conversation address; the separately supplied generation guard rejects a stale live binding before effect.

## Session lifecycle

1. Host resolves a provider binding and launches the provider ACP process under its provider-session supervisor. The supervisor, not the caller transport or call-local SDK object, owns the child process and provider connection.
2. Initialize ACP and record the advertised capability set, authentication state, provider runtime identity, and endpoint binding.
3. Create or load a provider conversation with Router `cwd`, requested policy, and provider-supported MCP binding. Return an endpoint-scoped `SessionRef` that can be used by later prompt/load calls.
4. Consume ACP updates internally to settle the correlated operation. Agents post ordinary MCP board messages; existing listen observes those messages. Provider updates produce no automatic board posts.
5. Before the first mutating operation, the supervisor requires Router qualification that caller detachment preserves its owned provider connection. If absent or unverified, the operation fails before provider mutation. After admission, caller disconnect closes only the caller attachment; the supervisor keeps provider work alive and exposes the same binding for later attach/load.
6. On explicit cancel, send provider cancellation and settle only when the provider reports a terminal result; otherwise retain cancellation uncertainty.
7. On crash, timeout, EOF, or response loss, classify stage/effect and write metadata to the Host-owned operation record. The record is persisted through the metadata-only service store defined below, contains no content or transcript, and survives Host restart. Reconciliation queries the same operation ID/provider binding; `confirmed` requires exact provider evidence, otherwise the record remains `unresolved` or becomes `not_reconcilable`. Terminal reconciled/not-reconcilable records are pruned only after the configured retention window. No replacement operation is issued automatically.

### Lifecycle and concurrency

The supervisor admits one active prompt/load per conversation. A second mutation
returns busy. Cancellation is an out-of-band command to that active operation,
not another prompt-slot occupant. The supervisor's select loop continues reading
commands and permission replies while awaiting the SDK prompt future; it never
holds the conversation mutation lock across that await. Explicit cancel carries
target, operation ID and binding generation, so it cannot hit a later prompt.
A duplicate cancel returns the recorded cancellation-request state without a
second cancellation dispatch. The first correlated terminal outcome settles the
operation; later updates cannot overwrite it.

Caller detach removes only its waiter. It does not drop the SDK request future,
send session/cancel, close provider pipes, or terminate the child. This is a
Router ownership property verified by integration tests, not a presumed ACP
`detached-work` field. Provider EOF and Host shutdown are separate failures.
Host shutdown stops admission, persists pending dispatch uncertainty and settles
owned resources; survival across Host death is not promised. A later explicit
load may recover a provider-owned session, but never replays the interrupted
prompt or confirms its outcome merely from session existence.

## Settings and permissions

The request records three separate values: Router requested policy, provider-effective settings, and verification status. Permission outcomes are `allowed`, `rejected`, `unanswered`, or `unsupported`; authentication is a separate prerequisite with `authenticated`, `authentication_required`, or `authentication_failed`. `allowed` authorizes the requested action; it does not prove execution or success. None of these answers is a confinement guarantee. Claude settings layers and permission modes are resolved by the bridge/runtime. Cursor inherits its installed CLI configuration and project `.cursor` configuration; ACP does not establish a named profile or universal sandbox override.

## MCP setup boundary

For Router collaboration tools, the provider session receives an MCP binding only through its documented provider path. Claude receives MCP definitions through the `claude-agent-acp`/Agent SDK bridge; Cursor receives the supported project/ACP MCP configuration, with dashboard/team-only injection excluded. Separately, a Codex caller registers the Router endpoint with `codex mcp add <name> --url <manifest-url>`, then verifies it with `codex mcp get <name> --json` and `codex mcp list`. The skill uses the live manifest URL and server schemas rather than copying a static catalog. The endpoint remains loopback-only and no session JSONL path is introduced.

## Current and proposed call path

```text
Current PR1:
CLI/MCP -> collaboration-client -> advertised codex-local Unix ACP -> Codex adapter/app-server
             |-> ControlClient -> endpoint discovery and service operations

PR2 adds:
CLI/MCP -> collaboration-client -> Control socket -> service dispatch
                                      -> Host provider binding -> official Rust ACP SDK
                                      -> Claude bridge -> Claude Agent SDK/runtime
                                      -> Cursor `agent acp` -> Cursor runtime

Preserved edges: PR1 CLI/MCP adapters, SessionRef target validation, board/listen coordination.
Added edges: Host provider supervisor, dynamic external endpoint discovery, provider ACP callbacks.
Changed edge: collaboration-client selects a provider binding instead of assuming codex-local.
No edge: provider transcript/session JSONL -> Router; no public transcript/observe store.
```

## Proof obligations before implementation review

- Live initialize and create for each provider using the target installed/selected runtime.
- Prompt/update, explicit cancellation, caller detach, and response-loss uncertainty tests.
- Load/resume only for capabilities actually advertised by that runtime.
- Permission allow/reject/unanswered paths and separate authentication-required/failed paths with requested/effective/verified mapping evidence.
- Authentication-required and unsupported capability outcomes, separately from permission rejection.
- MCP collaboration binding proof for each provider path, distinct from Codex caller registration, without reading or storing provider transcripts.
- Host-owned endpoint-scoped SessionRef creation/discovery, detach/reconnect, exact-evidence reconciliation, operation-ID retention, and no-replay proof.
- Schema validation for CLI/MCP outputs and no-replay assertions after ambiguous outcomes.
- Separate evidence for provider callback policy and OS/filesystem sandboxing.

## Explicit non-goals

No provider transcript store, JSONL reader, task registry, automatic replay, automatic replacement, scheduling/timed wakes, remote MCP/auth layer, custom JSON-RPC implementation, or production Router restart is part of PR2.

## Caller-to-Host operation interface

Extend the existing typed Control schema/dispatch, not the native Codex carrier.
External conversation methods are `conversation/create`, `conversation/load`,
`conversation/prompt`, `conversation/cancel`, `conversation/operationShow`, and
`conversation/operationWait`, and `conversation/operationReconcile`. These names are the external-provider route; the
existing Codex ACP route and automation operation methods keep their semantics.
All public CLI/MCP mutations require a validated operation ID supplied by the outer consumer and forwarded unchanged through SDK and Control. MCP cannot allocate it server-side after tools/call starts. Local convenience generation must expose/retain the ID before dispatch. Mutations also carry exact endpoint/target,
binding generation when targeting live work, and existing creator/approver
identity. Service dispatch validates those identities before admission.

Submit returns operation ID, admission state, and target when already known.
The SDK's synchronous convenience operation submits then waits through
operationWait. Waiters are call-local; provider work is not. operationShow reads
metadata and survives restart. operationWait may return bounded ephemeral output
while that operation remains loaded, but no output is persisted; a reconnect
with no retained output reports output unavailable along with durable metadata.
No catch-up transcript, event cursor store, or prompt replay is supplied.

Host injects a provider-operation handle into service composition, following the
existing Host-backend injection pattern. Control dispatch forwards typed commands
through that handle. CLI and MCP share the SDK methods and schema projections.
Request-task cancellation drops only the waiter; the Host task owns the SDK
request future. Explicit cancellation is a separate command, including when an
approval reply is pending. Board messages remain agent-authored via MCP; raw ACP
updates never become board posts. Existing listen observes those board messages.

## Metadata-only storage and crash ordering

Add a focused SQLx store under collaboration-service, opened by Host at
`<service-directory>/provider-operations.sqlite`. This is the concrete realization
of the approved Router-owned metadata record, not a provider transcript store
or generic task system. Keep it separate from automation-storage: existing
operation_receipts holds canonical requests, results and errors, and its public
method enum is automation-specific. Do not copy that payload schema or expand
scheduling to accommodate conversation records.

A native migration creates one operation table with an allowlist of operation
ID/kind, service/endpoint/binding identity and generation, nullable target,
admission/dispatch/terminal stage, effect, reconciliation state, and timestamps
needed for retention. Store validated enum/newtype fields; no prompt, payload
hash, reply, tool output, arbitrary error JSON, canonical_request or transcript
column. Free-text provider diagnostics remain transient and redacted.

Admission transaction inserts the operation before any provider call. Commit a
may-have-dispatched marker before sending a mutating ACP request. Commit the
known target as soon as obtained, then commit correlated terminal metadata.
Database write failure before dispatch returns no-effect; after dispatch it
cannot be reported as no-effect. Reopen reclassifies dispatch-without-terminal
as unknown; it never launches/replays a prompt. An admission record with no
dispatch marker can safely remain no-effect. Duplicate IDs are inspection-only
and never compare/store prompt content.

The store has a bounded maintenance batch (at most 1000 rows) and Host setting
`provider-operation-retention-days` (positive integer, default 60 days) for
terminal rows. Unresolved rows are never automatically pruned. There is no
new daemon or cleanup API. Maintenance cannot delete an active waiter/operation.
The selected timeout and retention values are exposed in effective Host config.

## Focused correction proof

- Caller drops CLI/MCP attachment during a held prompt; provider pipe and SDK
  future remain live. Explicit cancellation over another Control connection
  reaches the held prompt before completion and cannot cancel its successor.
- Inject process exit after admission, dispatch marker, provider effect and
  terminal commit. Reopen the actual SQLx database and inspect the original ID;
  assert the expected no-effect/unknown/terminal state and zero redispatches.
- Submit the same ID with different prompt content; it only returns metadata.
  Search actual rows/WAL and Router logs for sentinel prompt/result/tool output;
  none may be retained. Provider-owned persistence is outside Router's store.
- Test real populated migration/reopen, retention cutoff, unresolved exclusion,
  and invalid stored enum decoding. Keep SQLx checked-query metadata current.

### Explicit reconciliation call

`conversation/operationReconcile` takes the existing operation ID and reads its
metadata through service dispatch. Host then uses a supported read-only provider
evidence query, if one is available for that exact operation/target, and commits
the resulting reconciliation state. Exact correlated terminal evidence can
confirm the outcome; insufficient evidence leaves it unresolved; a provider with
no exact-evidence query returns not-reconcilable. A transport outage alone leaves
it unresolved, not confirmed or safely idle.

The call never submits create, prompt, load, or cancel to recover evidence, never
launches a replacement provider, and never reads provider session files. No exact
query is presumed available for either provider. operationShow remains a metadata
read; operationWait waits on an already-owned operation. Neither triggers
reconciliation implicitly. Not-reconcilable records retain unknown effect and do
not unlock the affected conversation for new mutation.

### Corrected boundary verification

The HTTP MCP caller chooses and retains an operation ID, submits it, loses the
response after dispatch, and uses a fresh operationShow call for that same ID.
Assert zero resubmissions. Exercise the analogous CLI caller-known ID path.
After Host restart, explicitly invoke operationReconcile and exercise exact,
insufficient and unsupported evidence using realistic contracts; none may issue
a mutating provider call. Verify that ACP updates alone produce zero board posts,
while a real agent-authored MCP board post remains visible through listen.
