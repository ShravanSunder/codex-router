# PR2 External ACP Specification

## Domain entities

### E0 Provider Endpoint Binding

Identity is a Router endpoint binding consisting of provider kind, provider runtime/bridge identity, configured launch binding, and an opaque provider conversation identifier. The binding is represented through the existing endpoint-scoped `SessionRef`; the provider identifier is never interpreted as a Codex thread ID. A dynamic endpoint advertisement contains endpoint ID, provider kind, runtime identity, transport, capability/evidence set, availability, service epoch, and binding generation. A binding is valid only after the provider endpoint has initialized and advertised the required capabilities, including Router-qualified caller-detach support for mutating operations. Caller detach leaves the Host-to-provider connection intact; no nonstandard ACP capability field is assumed.

### E1 Provider Conversation

Identity is the provider's conversation identifier together with the provider instance and Router provider binding. It represents one provider-owned conversation that Router can address without importing its transcript.

Conversation execution states are `idle`, `running`, `cancel_requested`,
`closed`, and `uncertain`. Creation or successful load enters idle; an admitted
prompt enters running; a correlated terminal result returns running or
cancel_requested to idle. Explicit close enters closed; ambiguous provider loss
enters uncertain. Settlement belongs to an operation, not to the lifetime of its
conversation. Attachment is independently attached or detached: caller departure
does not change execution state. Uncertain work cannot accept a new mutation
until supported reconciliation establishes safety. Late updates remain correlated
to their original operation and cannot alter a later operation.

### E2 Provider Capability

Identity is a capability name returned or proven by a specific provider runtime version. A capability may be `supported`, `unsupported`, or `unverified`.

### E3 Effective Provider Settings

Identity is one conversation's observed provider configuration. It records requested Router policy separately from effective provider settings and mapping status.

### E4 Operation Outcome

Identity is one Router operation attempt with a validated operation ID supplied and retained by the outer consumer before submission. CLI and MCP mutation inputs carry that ID explicitly; the MCP server must not silently allocate it after accepting tools/call. A local CLI or in-process helper may allocate it before sending only when its caller retains the ID across response loss. It records provider binding, stage, effect (`none`, `applied`, or `unknown`), target when known, and reconciliation state. The record contains metadata only; it has no prompt, reply, tool output, or transcript. An unknown outcome is inspectable by operation ID and is never replayed automatically. Operation reconciliation is `unresolved` until the exact provider operation is independently evidenced, `confirmed` only when the provider reports the same operation/target outcome through a supported query or correlated terminal update, or `not_reconcilable` when the provider offers no exact evidence path.

### E5 Collaboration MCP Binding

Identity is one provider session's configured MCP server binding. It contains only the endpoint/configuration needed to expose Router collaboration tools; it does not contain provider transcripts or session files.

## Normative obligations

1. **ACP transport:** Router MUST use the official Rust ACP client SDK over provider stdio/ACP transport and MUST preserve typed request correlation, updates, permission callbacks, and cancellation semantics.
2. **Create and prompt:** Router MUST expose independent create and prompt operations and MAY expose create-and-first-prompt composition where the provider path supports it. A delivered provider conversation identifier MUST be retained on post-create failure.
3. **Load/resume:** Router MUST expose load/resume only when the provider advertises and successfully accepts that operation. Cursor's current ACP evidence supports load/list but does not establish fork or close; Claude adapter evidence supports load/resume/fork/list through its bridge.
4. **Detach versus cancel:** Caller transport loss MUST detach without issuing cancellation. A provider binding that cannot preserve mutating work after detach MUST be rejected before mutation. An explicit cancel MUST issue the provider's cooperative cancellation path and report that completion is not guaranteed.
5. **Uncertainty:** A lost or ambiguous response after a potentially applied operation MUST settle as `unknown` with any known target. Router MUST retain that result for explicit reconciliation and MUST NOT retry or auto-replay it.
6. **Reconciliation:** Router MUST expose inspection/reconciliation of the same operation ID and MUST report `unresolved`, `confirmed`, or `not_reconcilable` without issuing a new provider operation automatically. Unknown metadata survives Host restart; unresolved records are not automatically pruned. Terminal records may be pruned after the configured retention window; inspection then returns not-found, never permission to resubmit the original operation.
7. **Permissions and auth:** Router MUST distinguish permission outcomes (`allowed`, `rejected`, `unanswered`, `unsupported`) from authentication prerequisites (`authentication_required`, `authenticated`, `authentication_failed`). Router MUST expose requested policy, provider mapping, and effective/verified status separately. A provider permission callback MUST NOT be reported as filesystem or process sandbox proof.
8. **Updates:** Router consumes ACP updates internally to settle operations. Agents publish ordinary collaboration messages through MCP; board/listen observes those messages, not automatically copied ACP updates. It MUST NOT create a public transcript or observation store for this purpose.
9. **MCP collaboration tools:** Router MUST pass its collaboration MCP binding only through a provider-supported path: Claude bridge MCP definitions or Cursor's documented project/ACP configuration. Codex's `codex mcp add` registration is a separate caller setup path. Provider-owned configuration remains provider-owned; dashboard/team-only or undocumented injection paths are unsupported.
10. **No session JSONL:** Router MUST NOT read, tail, parse, copy, or persist provider session JSONL or transcript files.
11. **Schema and skill guidance:** The Router skill MUST explain MCP registration and verification using the running server's schema and descriptions. It MUST NOT duplicate the entire tool catalog or claim provider capabilities that are only inferred.

## Capability baseline

| Capability | Claude bridge | Cursor ACP | PR2 status |
|---|---|---|---|
| create/new | source-supported; bridge/runtime launch proof pending | initialize/create path live-probed only after auth; authenticated conversation proof pending | unverified until exact runtime proof |
| prompt/updates | source-supported; bridge/runtime launch proof pending | documented path; real prompt/update proof pending | unverified until exact runtime proof |
| load/resume | source supports load/resume/fork/list | load/list advertised; fork not established | conditional and runtime-qualified |
| permissions | source maps ACP requests from Claude modes; callback proof pending | callback documented; real allow/reject/unanswered proof pending | mapping disclosed, runtime-qualified |
| cancel | source-supported; runtime proof pending | documented ACP cancellation; runtime proof pending | cooperative and runtime-qualified |
| MCP | bridge converts supplied definitions | project/ACP MCP capability advertised; execution proof pending | provider-path proof required |
| model/effort/sandbox | uneven, runtime-dependent | not established uniformly | unsupported or unverified unless proven |

## Admission, caller attachment, and cancellation

Before provider mutation, the caller knows the operation identity and the service
has committed its metadata record. If persistence fails, no provider work is
sent. An operation identity already present is inspect-only: submitting it again
returns its recorded state and does not dispatch any payload, even a changed one.
The record contains no prompt digest or canonical request. This is not an
idempotent replay service.

Caller connection closure and MCP request cancellation detach that caller from
Host-owned work. An explicit conversation cancellation targets the currently
active operation and its binding generation. It may run while the prompt is
awaiting the provider; it never queues behind prompt completion. Cancellation
acceptance is not cancellation completion. A terminal provider result observed
first wins; a late cancel cannot affect a subsequent prompt.

Conversation identity, active-turn state, and caller attachment are distinct.
An idle conversation can receive a later prompt after settlement. A detached
caller does not make the provider turn idle. While an operation is uncertain,
new mutating work on that affected binding is refused until explicit supported
reconciliation establishes safety; no automatic replacement is permitted.

On Host restart, committed admission with no dispatch marker is no-effect;
a committed dispatch marker without a committed terminal outcome is unknown.
That classification intentionally includes the crash window between recording
intent and actually sending the request. Successful session load or a similar
answer does not prove the earlier operation completed.
