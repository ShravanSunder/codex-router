# codex-router Quota Routing Safety Spec

Date: 2026-06-26
Status: reviewed once; accepted findings addressed

## Product intent

`codex-router` is an account router for Codex. Its job is to choose an upstream
OpenAI account, attach that account's OAuth credential, and keep Codex protocol
traffic pass-through except for the minimum routing/auth/affinity metadata needed
to make account selection safe.

The quota system must make the router usable for real long-running Codex work:

- Do not send a new Codex request to a weak account merely because the account
  has nonzero weight.
- Do not expose one account's quota exhaustion to Codex while another configured
  account can still serve the request.
- Do not block startup on quota refresh. Start from cached quota state, then
  refresh in the background.
- Make every routing choice inspectable through `codex-router quota` and the
  local OTEL/Victoria stack.

## Current-state evidence

- The current runtime still owns a process-lifetime smooth weighted selector:
  `RouteBandWeightedSelectors` in
  `crates/codex-router-proxy/src/account_selection.rs`, and the weighted
  selector adds positive weight to every eligible candidate before choosing a
  winner.
- The burn-down assessment produces `preferred_next`, but it also returns
  `weighted_candidates`; current runtime selection feeds those candidates into
  `WeightedDeficitSelector::select`.
- `WeightedDeficitSelector` increments every candidate's current weight and
  eventually selects lower-weight candidates when their deficit accumulates.
  A weak account with weight `1` can therefore still be selected later.
- The current runtime selector first checks previous-response affinity, then
  account hold cooldown, then weighted deficit.
- Active reservations exist in process memory and are mirrored to sqlite through
  `ActiveClientLeaseReporter`, but correctness depends on every stream lifetime
  retaining and releasing the guard.
- Active reservation pressure is cost-based, not count-based. WebSocket and
  HTTP/SSE requests can carry different pressure costs, while the CLI may
  display sqlite lease counts.
- The sqlite active lease table is currently a status/proof mirror. Runtime
  selection reads process-local reservation books. This can diverge unless the
  contract names which source owns routing truth and which source owns display
  truth.
- Current reservation ids are local counters. Durable active-lease mirrors must
  avoid collisions across process restarts or parallel router processes.
- Codex creates a turn-scoped `ModelClientSession`; a physical WebSocket can be
  reused across turns, but the turn-state token is scoped to one turn. It is not
  a generic connection pool the router may switch mid-message.
- Codex has test coverage for reconnecting after a
  `websocket_connection_limit_reached` error, and for falling back to HTTP after
  WebSocket retries are exhausted.

Evidence anchors:

- `crates/codex-router-selection/src/weighted_deficit.rs`: smooth weighted
  deficit selection.
- `crates/codex-router-selection/src/burn_down.rs`: burn-down assessment,
  selected pools, weighted candidates, and preferred next.
- `crates/codex-router-proxy/src/account_selection.rs`: runtime selection order,
  active reservation books, account holds, and reservation spans.
- `crates/codex-router-state/src/sqlite.rs`: selector windows, quota history,
  exhausted marking, and active lease storage.
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/src/client.rs`:
  Codex `ModelClientSession`, WebSocket connection reuse, and turn-state
  metadata.
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/tests/suite/client_websockets.rs`:
  tested `websocket_connection_limit_reached` reconnect behavior.
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/tests/suite/websocket_fallback.rs`:
  tested WebSocket fallback behavior.
- `/Users/shravansunder/Documents/dev/open-source/ai-harness/codex/codex-rs/core/tests/suite/turn_state.rs`:
  tested same-turn sticky state and cross-turn reset behavior.

## Non-goals

- Do not switch accounts per WebSocket message.
- Do not parse, validate, transform, summarize, or reinterpret Codex payloads
  except for the already-approved routing/auth/affinity boundary.
- Do not change Codex CLI.
- Do not disable WebSockets.
- Do not maintain parallel old/new selector behavior. This is a hard cutover.

## Requirements

### R1. Strict quota choice

`preferred by quota` must mean the next non-affinity request will select that
account unless fresh state changes before the request.

The selector must not use smooth weighted deficit as the final selector for quota
routing. It may compute tie-break data, but the final choice must be deterministic
from the current decision model.

The decision model must:

- exclude disabled accounts and accounts without active credentials;
- exclude accounts with exhausted or ineligible quota windows;
- keep unknown quota out of normal routing when any known usable account exists;
- prefer accounts that preserve weekly quota health;
- account for 5h pressure, weekly pressure, reset timing, projected runout, and
  active load;
- use stable tie-breakers so repeated tests are deterministic.

### R2. Load-aware burn-down

The selector must score active load as reserved future burn, not just past
historical burn.

Inputs:

- current 5h remaining percentage;
- current weekly remaining percentage;
- reset times for each window;
- reset credits available when provider data exposes them;
- historical observations from the current and recent reset segments;
- active reservations and client leases per account;
- request route band and request cost class.

Output:

- selected account;
- all rejected/held account reasons;
- limiting window;
- projected runout;
- active-load contribution;
- reason code stable enough for tests and OTEL queries.

### R3. Active connection accounting

Active client counts must be correct enough for both selection and operator
display.

The router must track:

- active WebSocket reservations by account and route band;
- active HTTP/SSE reservations while the streamed response is alive;
- reservation acquisition, re-reservation, release, and stale-purge events;
- sqlite mirror state for `codex-router quota`;
- OTEL spans/metrics for live diagnosis.

Runtime routing must consume one explicit active-load source of truth. If the
runtime source remains process-local reservation books, `codex-router quota`
must label sqlite lease counts as an async mirror and include freshness. If
runtime routing moves to durable leases, the selector must treat stale leases
and process crashes deterministically.

Decision for this spec: runtime routing uses process-local reservation books as
the active-load source of truth so selection does not block on sqlite. SQLx
active leases are the cross-process status/proof mirror for `codex-router quota`,
operator diagnostics, and crash/stale cleanup. Mirror write failures must be
observable and must not let the CLI present stale counts as exact.

`codex-router quota` must not invent client counts from stale sqlite rows without
showing freshness. If active-client state is unavailable, the CLI must state that
plainly instead of showing misleading counts.

`codex-router quota` and runtime routing must agree on live-load effects. The
same active pressure that changes runtime selection must also change the quota
command's selected account, or the quota command must explicitly label its
selection as ignoring live load. Preferred behavior is agreement.

Durable reservation identifiers must be unique across process restarts and
parallel router processes. Local ids such as `reservation_1` are not acceptable
as durable primary keys without a process/run prefix.

### R4. Codex-safe account exhaustion

Codex must not see one account's quota exhaustion while any other configured
account is usable.

When an upstream account returns a quota-exhaustion signal before the router has
completed a response:

1. mark that account/route-band quota as exhausted or suspect in sqlite;
2. release/retire its active reservation;
3. choose another usable account if available;
4. retry or reconnect only through behavior that Codex is known to tolerate.

Downstream commit boundary:

- Invisible retry/reselection is allowed only before provider response bytes are
  committed to Codex.
- After any HTTP/SSE response bytes are committed downstream, the router must not
  start buffering or interpreting arbitrary provider payload to hide errors.
- A post-commit retry is allowed only if a future spec explicitly authorizes a
  minimal Responses error-envelope detector with byte/time bounds, privacy
  review, and proof that it does not violate pass-through behavior.
- Without that explicit detector, post-commit provider errors are handled as
  pass-through transport/provider behavior, while the router updates quota state
  for future requests when it can classify the failure safely.

Suspect exhaustion state:

- `suspect_exhausted` means the account is non-selectable for new work, affinity
  reuse, and account hold reuse for that account and route band.
- It remains non-selectable until a successful refresh clears it, a known reset
  time passes and refresh/probe succeeds, or a bounded TTL expires into
  `unknown_needs_probe`.
- It must have a stable reason code and persisted state field so reconnect or
  retry cannot choose the same suspect account again by accident.

For WebSockets, the implementation must be based on Codex source-backed
evidence. The current candidate is the Responses WebSocket error frame:

```json
{
  "type": "error",
  "status": 400,
  "error": {
    "type": "invalid_request_error",
    "code": "websocket_connection_limit_reached",
    "message": "Responses websocket connection limit reached (60 minutes). Create a new websocket connection to continue."
  }
}
```

This candidate is allowed only after a router-owned integration test proves that
installed Codex reconnects through the router and completes the turn without
surfacing account-specific quota exhaustion.

Reconnect attempts must be bounded. The router must keep a per-request or
per-connection attempted-account ledger for quota/exhaustion rotation so a retry
cannot choose the same exhausted or suspect account again. If every configured
account has been attempted and no usable account remains, the router must stop
retrying and surface the router-level all-accounts-exhausted error.

The router must not use WebSocket close reason text as a reconnect contract.
Codex treats close as a generic WebSocket closure and does not parse close reason
text for account switching.

The router must not surface or synthesize `usage_limit_reached` as the switching
mechanism while another account is usable. Codex maps that class to rate-limit
user-visible behavior; it is not a safe account-rotation signal.

`websocket_connection_limit_reached` is a transport reconnect signal, not quota
truth. If used by the router, it must be used only as an internal compatibility
signal to make Codex reconnect so the router can reselect an account.

If all accounts are exhausted, the router may surface a router-level quota
exhausted error. The message must say all configured router accounts are out of
usable quota, not leak one account's provider payload.

### R5. Near-zero retirement

The router must avoid driving an account to hard zero when other accounts can
serve the work.

The selector must support a near-zero retirement threshold based on:

- remaining headroom;
- projected runout under current load;
- reset time;
- whether the request is a new turn or same-turn affinity continuation;
- whether all alternatives are worse.

Near-zero retirement must never break same-turn Codex sticky-state semantics.
If the only safe option is to keep a same-turn affinity connection until it
finishes, the router must do so and mark the account unavailable for new work.

### R6. Affinity and hold semantics

Previous-response affinity is stronger than ordinary quota preference but weaker
than hard account unavailability.

Rules:

- same-turn or previous-response continuation should stay on its owning account
  when that account is still usable or reserve-usable;
- if the affinity owner is exhausted or authentication fails, route safety must
  choose a Codex-compatible reconnect/retry path rather than silently using the
  wrong account;
- account hold cooldown is a stability mechanism, not a quota override;
- hold reuse must be blocked when the held account is materially worse than the
  current strict quota winner.

### R7. Quota CLI UX

`codex-router quota` must be the primary human command.

It must:

- show cached quota immediately;
- refresh in the background or inline after rendering the cached view;
- show an updated view after refresh;
- show exactly why the next account will be selected;
- show active clients per account with freshness;
- show reset credits;
- show stale/unknown/blocked states plainly;
- use the same strict selection engine as runtime.

### R8. Observability

The local Victoria/OTEL stack is the proof surface for runtime routing.

The router must emit scrubbed telemetry for:

- account selected;
- accounts rejected/held and reasons;
- active reservation acquired/released/re-reserved/stale-purged;
- WebSocket open/close/failure/retire signal;
- quota refresh success/failure;
- quota-exhaustion provider signal observed;
- all-accounts-exhausted router error;
- near-zero retirement decision.

Telemetry must prefer low-cardinality scrubbed dimensions:

- `account.hash` or `account.slot`, not raw account labels;
- `route_band`;
- `transport`;
- `selection.reason`;
- `quota.freshness`;
- `quota.window`;
- `quota.remaining_bucket`;
- `refresh.outcome`;
- `refresh.error_class`.

Forbidden telemetry:

- prompts;
- model payloads;
- auth headers;
- tokens;
- raw provider errors;
- raw filesystem paths;
- raw account labels;
- raw account ids in metric labels or log stream fields;
- raw reservation ids in metric labels or log stream fields;
- unbounded user/session identifiers as stream labels.

Required metrics:

- `codex_router_active_clients`: gauge by `account.slot`, `route_band`, and
  `transport`;
- `codex_router_account_selections_total`: counter by `account.slot`,
  `route_band`, `transport`, and `selection.reason`;
- `codex_router_account_rejections_total`: counter by `route_band` and
  `selection.reason`;
- `codex_router_quota_refresh_total`: counter by `route_band`,
  `refresh.outcome`, and `refresh.error_class`;
- `codex_router_websocket_events_total`: counter by `route_band` and
  sanitized `event.kind`;
- `codex_router_quota_remaining_bucket`: gauge by `account.slot`,
  `route_band`, `quota.window`, and `quota.remaining_bucket`;
- `codex_router_quota_pressure_bucket`: gauge by `account.slot`, `route_band`,
  `quota.window`, and `quota.pressure_bucket`.

All telemetry call sites must use one scrub helper for account, reservation,
path, and provider-error fields. Tests must exercise that helper directly and
through at least one log/span/metric emission path.

### R9. Storage boundary

All codex-router-owned production sqlite/state storage must use the repo's SQLx
state layer. No new `rusqlite` code paths are allowed, and no reachable
production `rusqlite` storage path may remain for codex-router-owned state.

This feature is the hard cutover point for codex-router storage. Production
account, credential metadata, quota, selector, active-lease, affinity, and
account-state reads/writes must go through SQLx. Existing reachable `rusqlite`
production paths for those surfaces must be removed or fenced as dead test-only
code before completion.

Reading external Codex-owned storage, such as Codex session metadata, must also
use SQLx when this repo opens sqlite directly. The router may call the Codex CLI
as a subprocess where appropriate, but it must not introduce a second Rust
sqlite client library.

State must include enough data to reconstruct the selection decision:

- latest selector windows;
- refresh status;
- quota history observations;
- reset credits;
- active client leases;
- process/run identity for durable active lease ids;
- account credential generation;
- near-zero/exhaustion marks.

### R10. Threat model

Assets:

- OAuth access/refresh credentials;
- account labels and account identifiers;
- provider quota and error details;
- Codex prompts, messages, tool outputs, and payloads;
- local router sqlite state;
- local OTEL/Victoria telemetry.

Entry points:

- local loopback HTTP/SSE/WebSocket traffic;
- upstream provider responses and errors;
- quota refresh calls;
- `codex-router quota`;
- OTEL exporter and Victoria queries.

Trust boundaries:

- Codex payloads are owned by Codex and must remain pass-through;
- provider credentials are router-owned secrets;
- sqlite state is local router state and must not be treated as trusted if stale;
- OTEL/Victoria is local debug infrastructure and must receive scrubbed data
  only.

Required controls:

- producer-side telemetry scrubbing before OTEL export;
- no raw tokens, prompts, payloads, account labels, raw account ids, raw
  reservation ids, raw provider bodies, or raw paths in logs/traces/metric
  labels;
- stable scrubbed account hash/slot for correlation;
- negative canary proof for sensitive strings in logs, traces, and metric label
  sets;
- route safety errors must not leak one account's provider body when another
  account can serve the request.

### R11. Proof expectations

This feature is not complete without all layers below:

- unit tests for selector edge cases;
- unit tests for burn/runrate calculations;
- integration tests for SQLx history, reset credits, and active leases;
- WebSocket integration tests proving no payload validation beyond routing;
- installed-Codex mock E2E with three concurrent WebSocket clients;
- router-owned Codex reconnect test for the selected exhaustion/retire signal;
- `codex-router quota` fixture tests for cached + refreshed display;
- OTEL/Victoria positive proof query for a fresh marker showing selection and
  rejection reasons;
- OTEL/Victoria negative canary proof that tokens, raw account labels, raw
  account ids, raw paths, prompts, payloads, and provider bodies are absent from
  logs, traces, and metric label sets.

## Boundary map

```text
Codex CLI
  owns: turn lifecycle, retries, WS fallback, payload protocol
  sends: HTTP/SSE/WS requests

        pass-through except local auth / routing metadata
        ▼

codex-router proxy
  owns: local auth, route classification, account choice, OAuth injection,
        affinity lookup, quota-safe retry/retire signals
  does not own: prompt/tool/message semantics or Codex payload validation

        reads/writes routing state
        ▼

SQLx state store
  owns: accounts, credentials metadata, quota snapshots, selector windows,
        quota history, active leases, affinity owners

        emits scrubbed proof
        ▼

Victoria / OTEL
  owns: local traces, logs, metrics for fresh proof markers
```

## Open decisions for spec review

1. Is `websocket_connection_limit_reached` the only acceptable router-generated
   reconnect signal? Current evidence says clean close reason text is not enough
   and `usage_limit_reached` is unsafe.
2. Should near-zero retirement apply only to new turns, or also to same-turn
   continuations after Codex proves reconnect preserves turn state?
3. What default thresholds should ship first for near-zero and material
   difference? The implementation plan may start with explicit constants and
   tests, then tune after live evidence.
