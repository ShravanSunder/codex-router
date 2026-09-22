# Quota Allocation Program Design

Date: 2026-09-18; revised 2026-09-20
Identity: `quota-allocation-program-design-2026-09-18`
Requirements: [Quota Allocation Requirements](2026-09-18-quota-allocation-requirements.md)
Specification: [Quota Allocation Specification](2026-09-18-quota-allocation-specification.md)

## Structural boundary

Quota policy remains owned by the existing projection, burn-down assessment, and account-selection path. Add only the S-QA-08 initial-admission classification before normal ranking. Weekly-only parsing/replacement stays authoritative. Add process-shared soft-affinity publication and established-socket activity renewal; hard previous-response ownership still takes precedence over soft/new assignment. SQLite remains asynchronous durable fallback.

The worktree baseline is `5b563c77e702a40a3f20c8f04234a4e5307aedea`. No cache/token module exists in this baseline or in the inspected original checkout at `fde60bf`. The target cache below must be implemented against current code; unavailable historical files are not dependencies. Current source anchors below are relative to this repository.

## Existing foundation and change boundaries

| Current source and behavior | Target delta |
| --- | --- |
| `quota_refresh_provider.rs::quota_response_from_window_pair` accepts weekly-only responses; `sqlite.rs::record_refresh_success_and_replace_selector_windows` atomically replaces windows | Preserve; add regression proof for missing/returning short-term window |
| `selection_projection.rs::estimate_window_burn_rate` produces no exhaustion time for zero/missing burn | Preserve honest forecast; assessment gains explicit initial-admission classification |
| `burn_down.rs::weekly_window_is_drain_pool_candidate` requires calculated runway | Keep normal rule; a separate initial-admission tier covers S-QA-08 cases |
| `account_selection.rs::select_upstream_account` holds selection lock, assesses eligibility, resolves hard owner, reads soft affinity from SQLite, queues writes | Keep eligibility/error precedence; add shared live lookup/publication and activity handle |
| `server.rs::LoopbackRouterRuntime` creates request selectors; `websocket.rs` pump forwards frames | Share one cache across both transports; carry handle into pump and touch after successful request send |
| `maintenance_actor.rs` discards repository results; lifecycle `run_maintenance` exits on error | Log scrubbed failure at existing actor; lifecycle retains existing interval and continues after failed iteration |

The source of session affinity is global `session_id` within this Router process and durable table, as today. Route band scopes quota assessment and reservations; it does not partition soft-affinity identity. Cross-process shared-memory consistency is not promised.

Entity binding: E-QA-01 uses the existing `AccountId` and account repository; E-QA-02/03 use the global session identity, new runtime affinity cache and existing affinity table; E-QA-04 stays with the previous-response ownership repository; E-QA-05 uses existing quota window facts and reset-segment projection; E-QA-06 uses the reservation book and existing load mirror; E-QA-07 remains with each retention owner below. No entity introduces a schema.

## Owners

```text
LoopbackRouterRuntime
  owns one shared SessionAccountAffinityCache
  shares the existing selection lock and DbWriteActor

AsyncRepositoryBackedAccountSelector
  checks eligibility, then hard ownership before soft/new selection
  reads fresh cache, then SQLite fallback and reconciliation
  invokes the common assessment, including initial-admission priority
  publishes the selected owner before releasing selection serialization

SessionAccountAffinityCache
  owns live session owner, last-seen time, strict freshness, pruning,
  monotonic timestamps, and pointer-identity owner tokens

SessionAffinityActivityHandle
  can renew only its current token

AsyncWebSocketTunnel
  forwards bytes unchanged and touches the current handle only after
  successful recognized response.create forwarding

DbWriteActor and SQLite repository
  persist publication and valid activity asynchronously

Existing quota/status projection and formatting
  consume the shared assessment and RoutingReason
  show load source/freshness and initial-admission reason
```

No second store, scheduler, retry loop, schema, or operator surface is introduced.

### Why these owners

The existing asynchronous SQLite writer cannot publish ownership immediately to another selector. Waiting for its write would put storage latency on forwarding. A shared memory owner plus the existing writer satisfies immediate visibility and bounded durability without another store. Live state can be lost on crash; restart uses the latest persisted row.

Initial admission belongs in the pure burn-down assessment shared by runtime and quota/status, not in a proxy-only fallback. Otherwise the operator's preferred account would disagree with actual assignment. It uses the existing reservation book rather than adding persisted probe state or background requests.

## Selection flow

1. Acquire the existing selection lock. Read live reservation counts, project quota and compute assessment; preserve existing admission/state/empty-pool errors.
2. Resolve `previous_response_id` on supported routes. A found eligible owner is selected; lookup failure cannot fall through to another account.
3. When no hard owner applies, read fresh soft affinity. On miss, await existing SQLite lookup, then reconcile under the cache mutex with the current live entry.
4. Revalidate the chosen affinity owner's availability and request exclusions using the current assessment. Expired or unavailable soft ownership falls through.
5. For new work, select the shared assessment's initial-admission winner if any, otherwise its existing normal winner. Both paths use the same quota/status engine.
6. Reserve the selected account through the existing guard before releasing selection serialization. Publish soft ownership for the session and return a handle bound to that ownership token. Queue existing persistence while serialized; forwarding never awaits the write.

The step-1 quota projection is required even for sticky/hard owners to validate eligibility. The earlier idea to move hard lookup before all quota evaluation was a change in error precedence, not necessary to prevent silent fallback; this target preserves the current contract.

Publication preserves monotonic timestamps. Same-owner publication reuses its token. An owner change creates a new token and invalidates older handles.

### Initial-admission classification

The pure assessment computes S-QA-08 qualification from the input's fresh quota facts, configured floor, burn/runway evidence and active count. Hard exclusions, stale/unknown quota, exhausted windows, known failed short-window guards and known positive-burn weekly runway below 900 seconds disqualify it. A reported fresh positive short window with no forecast is permitted; it is not labeled a passing forecast.

Keep the ordinary per-account assessment and near-zero retirement pass as the baseline for all accounts. Before selecting a pool, apply initial admission to qualifying accounts: set availability to `Usable`, retain or restore `Some(weight)` using the existing weight formula, including zero; never introduce a minimum-one weight, and carry an `InitialAdmission` priority variant. The exception supersedes both reserve classification and the built-in below-5% retirement hold for these accounts only. Keep the `InitialAdmission` priority unset during ordinary retirement comparisons and apply it only after that pass, so the exception does not accidentally retire or promote unrelated peers. No configured floor is bypassed.

Then run the existing pool selection and candidate collection. A qualifying account is in the Usable pool, including when it was previously Reserve or Retiring. Nonqualifying accounts retain ordinary availability; their reason is recomputed from the resulting pool as today: nonpreferred Usable peers are `AvailableSamePool`, Reserve peers are `HeldReserve` when the selected pool is Usable, and Retiring peers remain `RetiringNearZero`.

The first comparison in `candidate_priority_cmp` is the initial-admission tier, before `compare_weekly_drain_pool`. Within that tier compare weekly reset ascending, remaining weekly quota descending, then account identity ascending. For two nonqualifying candidates, execute the unchanged ordinary comparator. The selected qualifying account receives `PreferredNearResetInitialAdmission`, serialized as `preferred_near_reset_initial_admission`; actual confidence, burn and projected exhaustion remain unchanged. Extend existing CLI reason formatting and telemetry string mapping for that new reason without adding a new surface or field. Also extend the non-exhaustive `quota_status_value_formatting.rs::routing_reason_is_preferred` allowlist: `quota_status_view_model.rs::normalize_degraded_projection_authority` must clear preferred labels for initial-admission rows when load authority degrades, just as for existing preferred reasons. Prove this through the status projection, including `routing` and `next_use`, rather than only exhaustive enum-format matches.

The existing reservation book changes within the same selection critical section. The next concurrent request therefore projects one active client and cannot use the exception for that account. On release, actual count returns to zero and the next request reevaluates; no new cooldown, schema, or synthetic client is added. HTTP reservations and WebSocket lifecycle accounting keep their current definitions. Runtime counts use the existing process-local reservation-book overrides; only accounts absent from that book fall back to the SQLite load mirror. CLI status uses its existing read-only mirror projection and keeps the load-source/freshness labels. Do not promise instantaneous cross-process agreement while that mirror lags. The selected initial-admission reason flows through the existing RoutingReason representation and quota/status projection/formatting, with no additional status surface. A merely open inactive socket's treatment is whatever the existing active-client owner records; no inference from the affinity map is used.

### Affinity interfaces

`SessionAccountAffinityCache` is one runtime-owned `Arc<Mutex<...>>` map of session identity to account, last-seen and `Arc` ownership token. An activity handle holds cache, token, session identity and writer reference. It does not hold the selection lock.

- Lookup returns only an entry with saturating idle age `< 7_200`. On cache miss, existing SQLite read is permitted. Cache lock failure returns the existing bounded state-unavailable error for selection, not an invisible second owner.
- Reconciliation reads the current clock after the awaited read solely to compare freshness. Fresh live state wins; only a matching fresh persisted session row may seed the cache, preserving its persisted last-seen timestamp. Lookup and seeding never renew activity or enqueue a write. Reuse token only when the same cached account still owns the entry; subsequent selected-request publication may advance its timestamp.
- Publication and valid touches store `max(current_time, last_seen)`. Enqueue durable writes under the same cache mutex so concurrent updates preserve enqueue order. The existing writer queue remains bounded and best effort.
- Touch first checks exact token identity. A mismatched/absent owner is a no-op. A poisoned cache cannot be mutated by activity; report the existing scrubbed error/degradation category and do not retry inline.
- Prune opportunistically at most once per minute on selection. Remove expired entries only when no external current-token handle exists. Retained expired entries are still ineligible for a new lookup; a valid current socket may renew them through real activity.

The selected decision gains an optional handle. `server.rs` supplies the same cache and writer through the request runtime state to both HTTP and WebSocket selectors. The WebSocket owner/forwarding context carries that handle unchanged through tunnel setup into the local-to-upstream pump; previous-response recording remains separate.

Initial selection/reselection publishes last-seen before forwarding, preserving existing behavior for HTTP requests that later fail. Established WebSocket activity renewal requires successful forwarding. No new HTTP completion hook is required.

## WebSocket activity flow

The established WebSocket pump receives the token-bound activity handle in its per-connection forwarding context. Reuse the existing `is_response_create` top-level metadata classifier, forward the original bytes, and call `touch_if_current` only after its recognized `response.create` sends successfully. Do not introduce a second classifier, full-message JSON validator, or affinity-specific size ceiling. A large message with recognized metadata renews in the same way as a small message; malformed content after recognized metadata does not veto renewal. Unrecognized type metadata, ping/control traffic, unrelated messages, and failed sends do not touch the cache. This deliberately treats recognized forwarded activity as activity without claiming whole-message validity. The token check prevents stale sockets from reviving prior owners, including A-to-B-to-A.

## Consistency and failure behavior

The existing selection lock makes first publication visible before another selector chooses. It does not serialize an established WebSocket handle, so cache reconciliation after an awaited SQLite read is required: a live WebSocket touch may make a retained entry fresh while the selector is waiting, and that fresh live entry must win.

The existing affinity table treats `session_id` as the global affinity identity across route bands; route band does not create a second durable owner for the same session. An asynchronous write failure leaves the live process owner in place. Writes are eventual, so a process restart may recover an older row or no row; current-process cache state remains authoritative until restart. A SQLite read failure keeps the existing bounded state error. An account exclusion, hard owner, or eligibility change may supersede the initial owner. A stale token or absent entry is a no-op. Pruning may retain expired entries while a live handle exists for stale-handle rejection, but retained state is still ineligible at the strict boundary.

## Proof seams

The implementation must prove, through real selector/cache/writer boundaries:

- two concurrent first selections converge before durable persistence completes;
- cache lookup, reconciliation, and pruning all expire at 7,200 seconds exactly; lookup-only seeding preserves persisted last-seen, including at idle age 7,199;
- recognized request activity renews for small and large messages, including malformed suffixes after recognized metadata, while unrecognized/control traffic and failed sends do not;
- stale handles, pruning, and A-to-B-to-A cannot resurrect ownership;
- hard ownership precedes soft affinity and ordinary quota selection;
- active load and projected burn preserve June 28 scenarios with the Specification's explicit initial-admission supersession, S3m correction, and S3k fixture expansion;
- forwarding stays independent of slow or failed persistence;
- runtime/status agreement, freshness labels, pass-through, containment, and privacy remain intact.

## Current-to-target request sequence

```text
Client         Selector / existing lock        Live cache      SQLite / writer
  | request --->|                                 |                 |
  |             | quota projection + eligibility [unchanged] ------>|
  |             | hard owner resolution [unchanged precedence] ---->|
  |             | soft lookup [changed: memory first] ->|           |
  |             | miss: await durable fallback -------------------->|
  |             | recheck live cache [added] --------->|            |
  |             | affinity or common assessment winner             |
  |             | reserve + publish [added] ---------->|            |
  |             | bounded enqueue [existing async writer] --------->|
  | account <---| unlock; return handle [added]         |            |
  | forward to provider; do not await persistence      |            |

Established WS pump
  receive frame -> forward unchanged -> send succeeds
    recognized response.create -> token-check + touch -> bounded write enqueue
    send fails / control / stale token -> no renewal
```

Async database reads can fail before selection and propagate existing state errors. An established WebSocket touch may run while selection awaits SQLite because it uses only the cache mutex; post-await recheck must use the renewed live timestamp.

## Retention owners and failure recovery

`server.rs::enqueue_runtime_maintenance_hints` retains current cutoff math and cadence. `MaintenanceActor` matches `run_maintenance_hint` results: on failure emit maintenance class and safe route label without raw database error/session/account values, then remove the pending key exactly as on success. A later normal hint retries. Do not create another task or change the daily affinity guard.

`lifecycle-observation::LifecycleStore::run_maintenance` keeps its existing hourly interval and shutdown token. A failed `maintain` call emits a scrubbed warning and continues to the next tick. Do not mark the journal available or clear failure state manually; existing journal recovery remains authoritative. A later successful cleanup does not restore public journal availability: existing preparation is the recovery boundary. Initial journal preparation still fails closed on failure. The host already owns this task and joins it on shutdown.

Quota-history purge stays at refresh completion. Automation's existing minute worker already logs/retries and remains unchanged. SQL cutoff predicates and protected tables remain unchanged. Tests must demonstrate existing trigger wiring and error survival, without claiming that these opportunistic triggers enforce deletion while an otherwise idle process receives no connections.

## State and proof coverage

| State/event | Transition / owner | Proof observation |
| --- | --- | --- |
| No soft owner → selection | Reserve then publish before selector unlock | Second selector sees same eligible owner with writer blocked |
| Current A → B → A | New token on each owner change | Original A handle cannot touch the new A entry |
| Expired current entry with live handle | Lookup rejects at7200; handle may renew after forwarded request | Exact-boundary and idle-then-activity cases |
| Awaiting DB while WS touches | Recheck uses current clock and live entry | Older row cannot replace freshly renewed timestamp |
| Fresh idle quota, no runway | Initial-admission reason; count changes under existing reservation lock | First real client assigned; second sees nonzero count |
| Successful weekly-only refresh | Existing transactional window replacement | Removed short window no longer guards selection; failed refresh keeps evidence |
| Maintenance transient error | Existing owner logs and waits for next allowed trigger | Later lifecycle tick succeeds, no spin or task loss |

S-QA-01/02 map to selector ownership and eligibility; S-QA-03/05/06 to shared cache/token/writer and pump; S-QA-04/07/08 to provider parsing, state projection, pure assessment and reservation serialization; S-QA-09 to existing SQL repositories, maintenance actor and lifecycle loop; S-QA-10 to the existing quota/status projection and formatting owners. All U-QA-01 through U-QA-12 are covered through those Specification links.

Proof uses real selector/cache/reservation/writer/SQLite paths with fixture-owned databases, controlled time and loopback upstream transport. Delay or fail only the storage/transport seam being tested. Observe forwarded bytes, selected accounts, actual row counts and scrubbed failures. No production DB, live inference, service restart or external secret is required. Existing historical test results do not prove this target worktree.
