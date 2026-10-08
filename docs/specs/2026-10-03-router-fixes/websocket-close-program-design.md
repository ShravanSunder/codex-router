# Floor reconnect — symmetric cancellation-aware pump supervision

Realizes [Specification](websocket-close-specification.md) RW1–RW3 for [Requirements](requirements.md) U4. Retain the two existing pumps and their supervisor; change the supervisor's completion decision and make its retained cleanup reachable from the local pump's existing admission wait, without changing transport ownership or reconnect policy.

## How the required outputs survive completion order

The upstream-to-client Direction emits the existing reconnect signal and closes the client sink; the client-to-upstream Direction closes the upstream sink. The latter observes tunnel shutdown at its outer receive loop today, but not during turn admission. The upstream-to-client Direction produces that intent on quota/capacity close-after-send and observes hard/early floor tokens; it has no general tunnel-shutdown branch. Preserve its signal-before-Close producer sequence. The supervisor retains the successful counterpart, and the local admission wait must be escapable so retention leads to its required Close.

```mermaid
%%{init: {'theme': 'dark'}}%%
sequenceDiagram
  participant C as Client
  participant U as Upstream-to-client<br/>Direction
  participant S as Supervisor
  participant L as Client-to-upstream<br/>Direction
  participant P as Upstream
  participant F as Forwarding<br/>owner
  Note over L: May await turn admission
  U->>L: Existing tunnel shutdown intent
  U->>C: Existing signal then Close
  U-->>S: Successful completion
  Note over S,L: Retain survivor<br/>observe outer cancellation
  alt No external forced shutdown
    L->>L: Leave admission wait on shutdown
    L->>P: Existing Close write
    L-->>S: Joined result
  else Revocation or session shutdown
    S->>L: Abort and await
    L-->>S: Reaped
  end
  S-->>F: Both Direction handles settled
  F->>F: Drop registration and owned resources
  Note over U,L: Same completion rule<br/>in opposite order
```

![Preserve the surviving Close and remain responsive to forced shutdown](assets/websocket-close-path.png)

## Bindings and interfaces

| Entity | Semantic owner | Module/package home | Schema/type home | Boundary shape | Lifetime / convention |
| --- | --- | --- | --- | --- | --- |
| EW1 Tunnel | Existing duplex forwarding owner | `codex-router-proxy::websocket::duplex_forwarding` (existing) | Existing forwarding context, session registration and cancellation tokens | Local/upstream WebSocket streams; completion `Result<(), WebSocketTunnelError>` | Runtime only; RAII registration and owned JoinHandles |
| EW2 Direction | Its existing pump; supervisor owns its join/abort | Existing `pump_local_to_upstream` (admission wait modified), `pump_upstream_to_local` (producer path unchanged), `supervise_websocket_pumps` (modified) | Existing `JoinHandle<Result<(), WebSocketTunnelError>>` and tungstenite Message | Peer signal is existing text Message; Close is existing Close Message/close flush; task output is closed Result | Runtime only; Tokio task/stream ownership |
| EW3 Termination intent | Existing cancellation sources; supervisor decides survivor retention | Existing quota/early reconnect and tunnel/outer cancellation sources; supervisor (modified decision) | Existing CancellationToken values, existing WebSocketTunnelError | Reconnect shutdown token versus explicit revocation/session-shutdown tokens; no new wire field or stored reason | Existing runtime signals; no new enum/tag, persistence or task owner |

`supervise_websocket_pumps` keeps its current inputs and Result output. Signal production, sink close, reservation retirement and registration-drop ownership remain in their current modules. Local response.create admission adds only selection on its existing tunnel token before forwarding the waiting message; its actual admission policy and return type stay owned by AccountTurnAdmission. A small private helper may express the repeated survivor wait/abort rule inside the supervisor owner; it adds no public interface, task or state.

## The completion rule

| First observation | Supervisor action | Result authority |
| --- | --- | --- |
| Revocation or session shutdown | Abort and await both owned pumps | Existing forced-shutdown result |
| Direction finishes with error | Abort and await counterpart | Existing flattened first error |
| Direction finishes successfully with no tunnel shutdown intent | Abort and await counterpart | Existing successful ordinary completion |
| Direction finishes successfully with tunnel shutdown intent | Await counterpart while also selecting revocation/session shutdown | Remaining pump's flattened result, or existing forced-shutdown result |

### A retained Direction must reach its existing cleanup

| Direction / wait | Existing observation | Realization and preservation |
| --- | --- | --- |
| Client-to-upstream outer receive | Watches hard/early floor, tunnel shutdown, revocation and session shutdown | Keep current branch priority and Close behavior. |
| Client-to-upstream inside before_next_create | Only admission's hard/early floor and turn/assessment waits participate; quota/capacity tunnel shutdown can leave it parked | In this pump, select the whole existing before_next_create future against tunnel_shutdown.cancelled(), with shutdown priority when both are ready. On shutdown leave the pending admission future, close the existing upstream sink and return; do not forward the waiting response.create or create a new reservation. Keep the existing admission=true floor-close path. No new cancellation token, actor or admission policy. |
| Upstream-to-client quota/capacity close-after-send | This Direction sets tunnel shutdown, emits its existing client signal, then closes its sink and returns | Retain that producer sequence. Do not add a general tunnel-shutdown race which could discard the client signal. If this Direction remains alive after local-first completion, preserve its existing hard/early floor observation and in-flight signal/Close. |
| Either peer output awaits write readiness | Completion can remain pending on peer IO | Successful counterpart completion preserves it; supervisor selects existing revocation/session shutdown to abort and join. No timeout or unobserved peer-ack guarantee. |

Dropping the pending admission future releases its local lock/wait ownership; it does not roll back earlier read/assessment observations or invent a delivered terminal event. Any completed admission already accepted before shutdown retains existing semantics. Resource cleanup follows the settled Direction handles, not an invented admission rollback. On quota/capacity error-type frames, the existing terminal classifier intentionally may not release turn_activity; tunnel cancellation must therefore independently wake the local admission wait.

The final row applies symmetrically. External cancellation during that row aborts and awaits only the surviving handle; the first handle has already been consumed. No ended handle/future is repeatedly polled. Existing cancelled-join normalization remains authoritative.

The token represents the established tunnel cleanup intent, including existing quota-error and model-capacity close-after-send paths; do not narrow it to one test trigger or add a second floor policy. Successful first completion alone is insufficient to preserve an ordinary non-reconnect stream indefinitely.

## Current and proposed edges

| Current source edge | Proposed disposition | Why |
| --- | --- | --- |
| `forward_duplex_until_complete` splits streams, starts two pumps, awaits supervisor, drops registration | Intentionally unchanged | Singular task/resource ownership, RW2–RW3 |
| Both floor branches cancel tunnel token, then client-facing pump sends signal/Close and upstream-facing pump closes upstream | Intentionally unchanged | Preserve real output order/content, RW1/RW3 |
| Local-first successful shutdown awaits client-facing counterpart without watching outer cancellation | Changed: same retained cleanup with revocation/session-shutdown branches active | RW2 |
| Upstream-to-client-first completion unconditionally aborts client-to-upstream counterpart | Changed: apply the same error/shutdown-intent survivor rule as local-first | RW1–RW2 |
| Local response.create waits inside admission with no tunnel-shutdown observation | Changed: pump-level cancellation selection exits that wait into its existing upstream Close | RW1/RW3; prevents a retained pump from parking on unreleased turn_activity |
| Upstream-to-client quota/capacity signal and close-after-send | Intentionally unchanged; producer keeps its output ordering and owns its termination intent | RW1/RW3; a generic shutdown race here could discard the signal |
| Ordinary completion/error aborts and joins remaining work | Intentionally unchanged | RW3 |

Decisive current sources at 7a8cbd89: `websocket::duplex_forwarding` supervisor at :139–172; local floor/shutdown close at :204–218; client floor signal/Close at :283–302; quota-error shutdown and close-after-send paths at :335–337/:420–423; `transport_cleanup::close_websocket_sink_best_effort` at :42–52. Existing floor-switch supervisor cases deliberately exercise local-first completion only. They are useful preserved regressions, not upstream-first delivery evidence.

## Why this realization

The existing supervisor already retains successful local-first reconnect cleanup, so the symmetric rule supplies the missing output without a new scheduler, close protocol or grace-period policy. Watching existing outer cancellation inside the survivor wait keeps the established forced-shutdown boundary effective. Delaying or removing the client's reconnect signal would change the contract; leaving the unconditional abort loses upstream Close. Maintainers pay for one shared completion rule and deterministic interleaving proof. Revisit only for an established Close deadline requirement or a counterexample that the existing tunnel token does not represent cleanup intent; neither is inferred here.

## Failure, trust and proof seams

Peer IO can fail or remain unready. Existing close-error normalization and pump Result remain the authority. This design guarantees required cleanup is not prematurely aborted solely by successful counterpart completion; it does not assert delivery after peer failure or after explicit forced termination. No retries, frame replay, process changes, account/state copies, secret handling or new storage are introduced.

Real supervisor, pumps, Message encoding and close writes must participate in delivery proof. A controlled AsyncRead/AsyncWrite readiness gate around an isolated real duplex stream can force the order; it cannot fake successful Close delivery or return a manufactured pump result. Verify peer-observed frames after releasing readiness. Include a local response.create parked in actual admission during an active turn with pending floor intent, then deliver an error-type quota close-after-send and separately a model-capacity close-after-send through the real upstream pump. Release the actual upstream write gate; observe the original client payload/Close, upstream Close, joined handles and released registration/reservation without a terminal event unblocking turn_activity. Admission must not forward that waiting create after shutdown. Also retain both hard/early floor completion orders and forced cancellation during actual Close readiness. A supervisor-only synthetic JoinHandle case supplements cancellation-decision proof but is not actual peer-output proof. Keep time bounds around the harness, not sleeps that choose pump ordering.

| U | R / scenario | E | Owner | Interface | Shape and home | State | Failure | Proof |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| U4 | RW1 either completion order | EW1–EW3 | Existing supervisor and local admission-wait/output owners | supervise_websocket_pumps / pump_local_to_upstream admission selection / existing producer | JoinHandle Result and Message in websocket owner | Reconnect cleanup; retain survivor | First successful finish cannot abort required other output | Gated real Close/signal peer observation in both orders, including actual parked admission under quota and capacity triggers |
| U4 | RW2 forced termination while draining | EW1–EW3 | Existing supervisor | CancellationToken branches plus abort/await | Existing revocation/session-shutdown tokens and handles | Draining → forced termination → both joined | No live owned pump after completion | Hold actual close readiness, cancel externally, observe joins and released registration/reservation |
| U4 | RW3 preservation | EW1–EW3 | Existing forwarding/supervisor/signal/cleanup owners | Existing normal completion and error branches | Existing Message and WebSocketTunnelError | Ordinary completion/error or reconnect cleanup | No replay/new signal/error reclassification | Existing real normal/error/reconnect integration results/counters and parked-admission resource release for quota/capacity |

EW1–EW3 and VW1–VW3 are covered by these bindings and seams. Other U rows remain open. Executable close-race reproduction, full peer delivery and repeated CI evidence remain unverified until the pinned toolchain/debug proof is available.
