# Idle quota drain and floor protection specification

Date: 2026-09-25
Identity: `idle-quota-drain-specification-2026-09-25`
Requirements: [Idle quota drain and floor protection requirements](2026-09-25-idle-quota-drain-requirements.md)

## Domain and authority

This specification changes the account-selection, floor, and background quota observation behavior named in U-ID-01 through U-ID-07. It uses the existing September [quota allocation entities and contract](../2026-09-18-quota-allocation/2026-09-18-quota-allocation-specification.md): Account, Session, hard response ownership, soft affinity, Quota window, and Active client retain their identities. A reset credit remains separate provider inventory, never evidence that an account is currently routable.

| Identity / term | Same-instance rule and relationship | Observable states and invariant |
| --- | --- | --- |
| E-ID-01 Idle account | One existing Account with zero authoritative active clients for the request's route band at selection time; the same account becomes non-idle when its first client is reserved. | Idle is a current load fact, not an affinity-row or socket-presence inference. Re-evaluate after release. |
| E-ID-02 Configured weekly floor | One existing per-account weekly reserve policy. Absence is floor disabled; an enabled integer floor remains 1–15%. | The configured floor is the hard stop. A fixed three-point margin above it begins a best-effort graceful switch. Both thresholds remain distinguishable to operators. |
| E-ID-03 New independent request | A request with neither a resolved hard response owner nor eligible soft session affinity. | Only this request kind participates in ordinary or idle-drain ranking. A hard-owned continuation stays on its owner while above the configured floor and otherwise requires a fresh conversation. |
| E-ID-04 Burn evidence | The existing quota-window observations and confidence for one account, route band and reset period. | A first real client may be admitted while burn is unknown; no synthetic request or invented safe runway is recorded. |

## Observable contract

### S-ID-01 — Graceful switch and hard floor

When an account has a configured weekly floor `F`, its graceful-switch point is `min(100%, F + 3 percentage points)`. A healthy peer is another account that passes current runtime eligibility, including quota freshness, short-window and calculable 900-second runway guards, queue health and runtime exhaustion; it has no configured floor or remains above its own switch point. An unknown-burn peer qualifies only through the existing one-client idle admission rule. When any otherwise eligible account reaches its switch point but remains above `F`, new independent and eligible soft-affinity requests use only healthy peers when at least one exists; other candidates yield without losing hard-continuation eligibility. If no healthy peer exists, above-floor accounts retain ordinary fallback and ranking. This is a best-effort switch, not an availability failure. A hard-owned `previous_response_id` continuation remains on its owner while the owner is above `F`. These decisions still obey all other eligibility and safety guards.

At or below observed `F`, every request entering selection must stop using that account. A new independent or eligible soft-affinity request uses another eligible account, or fails closed if none exists. A hard-owned `previous_response_id` cannot be sent to a different account; it fails explicitly and the client must start a fresh conversation to use an eligible peer. With no configured floor, neither threshold applies. Hard exhaustion and ineligible or untrusted quota evidence continue to fail closed under their existing reasons.

The thresholds use the freshest admissible persisted provider quota observation available at selection. An established WebSocket does not rerun selection for each `response.create`. After a validated weekly quota refresh saves the burn-history observations needed for runway assessment and the selector window at/below the switch point, the Router marks that account's sockets for a graceful switch. It does not terminate an active turn. When the turn reaches a terminal result, or before an idle socket forwards its next `response.create`, the Router checks live peer eligibility; only a healthy peer causes the existing Codex reconnect signal and close. If no healthy peer is available, work may remain on the original account above `F` and the next turn boundary can try again. A fresh socket without a hard response owner can select the peer; a reconnect that still carries an account-bound `previous_response_id` may return to the old account while above `F`. At or below `F`, the Router promptly signals reconnect regardless of peer availability or active-turn state; a new socket cannot use the old account and fails closed if no peer is eligible. If a required history or selector-window write fails, the refresh reports failure and does not claim an enforced new observation or send the reconnect; the last persisted selector evidence remains authoritative until a later successful refresh or the existing freshness guard rejects it. A changed floor becomes authoritative to new selections through its saved policy; an existing socket receives the corresponding switch or hard-stop signal when the next successfully saved validated refresh observes the threshold. The Router does not read stored quota for every WebSocket frame.

This is best-effort reserve protection: already forwarded work, frames racing the reconnect signal, consumption between provider refreshes, and a failed required persistence write may cross `F`. The status surface shows both `F` and the graceful-switch point. The three-point value provides room above whole-percent observations and the normal refresh interval; it does not bound one request's consumption.

### S-ID-02 — No implicit near-zero retirement

Neither a below-5% weekly or short-window balance nor a projected runout within 30 minutes produces the old `Retiring` availability or `retiring_near_zero` reason. The configured floor, provider exhausted/ineligible evidence, reported short-window safety, current quota freshness, and the accepted calculable weekly runway guard remain independently binding. A zero-client account above its configured floor is not held solely because its percentage is small.

### S-ID-03 — Idle initial work without a burn estimate

A new independent request may give **one real client at a time** to an idle account whose weekly and any reported short windows are fresh, Eligible and positive; whose weekly remaining is above its configured floor; whose credentials and account are usable; and whose known guards do not fail. Inside the graceful-switch band it receives that client only when no healthy peer as defined in S-ID-01 can take the request. The account's weekly reset must be in the future, but need not be within 48 hours. The exception applies when the weekly candidate burn/runway is missing, insufficient, stale, or measured zero with no finite runway. A known calculable weekly runway below the accepted 900-second admission boundary cannot claim this unknown-burn exception.

The first reservation must become visible before a concurrent selection, so concurrent first starts do not all see zero clients on one account. While active count is nonzero, that account loses the idle exception; existing ownership and ordinary policy govern. After release, it may qualify again if burn remains unknown. Neither the exception nor the status surface claims that one client guarantees a measured slope: normal confidence still needs the existing observation span.

### S-ID-04 — Idle known-burn work beyond 48 hours

An idle account with a known usable weekly burn estimate, a future weekly reset beyond 48 hours, and a projected one-more-client runway meeting the accepted 900-second boundary can compete for new independent work. It must pass the same account, floor, weekly, and reported short-window guards, including graceful-switch peer preference. No reset credit is required. This uses real demand; it does not migrate an existing client or generate traffic.

Preserve the current priority and ordering for qualifying accounts inside the existing inclusive 48-hour drain pool, including its current initial-admission cases. When that pool has no eligible winner, compare all eligible farther-reset idle accounts before ordinary pool construction. Rank them ahead of already busy ordinary far-reset candidates by earlier weekly reset, then smaller positive remaining weekly balance, then higher burn confidence, then stable account identity. Promote only the winner to `Usable` for this assessment; a nonpreferred eligible idle peer retains its ordinary availability and reason. This ordering lets the 53-hour/7% account receive a new independent client ahead of the 97-hour/2% account; a subsequent independent client can consider the latter after the first reservation changes the former's active count.

An idle account with unknown burn that meets S-ID-03 participates in the farther-reset idle order without being labeled as known safe. An observed zero burn gives no finite exhaustion forecast. Re-evaluate from current reservations and quota before every new assignment.

### S-ID-05 — Status and failure behavior

Quota/status and runtime use one shared selection assessment for equivalent fresh quota and load facts. Status names an idle early-drain preference distinctly from the existing near-reset reasons, reports actual confidence and any unknown runway, and shows both the configured hard floor and graceful-switch point. Retired availability/reason values are removed from current output. A lagging SQLite active-load mirror may differ temporarily from runtime process-local reservations and must remain visibly degraded rather than claiming exact next-account parity.

The JSON account row retains `weekly_quota_floor_basis_points` and `weekly_quota_floor_percent` as the configured hard stop. It reports `weekly_quota_switch_at_basis_points` and `weekly_quota_switch_at_percent`, both `null` when no floor is configured and otherwise `F + 3` expressed in basis points and whole percentage points. Remove the unpublished `weekly_quota_effective_stop_*` fields because they described the wrong hard stop. A preferred farther-reset idle account reports `availability: "usable"`, `routing_reason: "preferred_idle_far_reset"`, and `preferred_next: true`; a nonpreferred eligible idle peer retains its ordinary availability and reason. The human quota detail says `switches at S% / floor F%` when configured and shows neither threshold when the floor is disabled. At/below `F`, exclusions keep the existing `excluded_weekly_quota_floor` machine reason; when a healthy peer is preferred, each yielding candidate remains hard-eligible above its floor but reports `routing_reason: "held_floor_switch"`. Without a healthy peer it retains its ordinary reason. Current JSON no longer emits `availability: "retiring"` or `routing_reason: "retiring_near_zero"`.

Hard previous-response ownership continues before soft affinity and new assignment above `F`. At/below `F`, a hard owner fails explicitly and requires a fresh conversation; a stale or floor-blocked soft owner falls through under the existing rules. Provider usage-limit exhaustion and short-window failures continue to use their existing containment; no reset credit is automatically inspected or consumed.

### S-ID-06 — Background quota observation cadence

The default enabled background quota worker begins an observation cycle immediately on Router startup and targets a new cycle every 180 seconds, measured from one cycle start to the next. Each cycle still attempts all enabled credentialed accounts regardless of client count or quota balance. If a cycle takes longer than 180 seconds, the next cycle begins after it finishes; an individual account's observation age is therefore not guaranteed to stay below three minutes. A provider failure is reported as failure, not a successful fresh observation. The existing explicit interval override and background-refresh disable option retain their meaning.

## Boundary examples and proof obligations

| Case | Required observation |
| --- | --- |
| 2% weekly, no floor, zero clients, reset 97h, known 86m one-client runway; healthy busy peer | Account is not retired and is eligible in the farther-reset idle group when no eligible near-reset winner exists. |
| 7% weekly, no floor, zero clients, reset 53h, known 8h32m runway; same peer | Account can be preferred for the next independent client; existing peer conversation remains on its owner. |
| Both idle examples together | Earlier reset 7% account wins first; after reservation it loses idle priority, so a second independent start can consider the 2% account. |
| Fresh eligible 4% weekly, zero clients, reset beyond 48h, unknown burn; healthy peer | One real initial client can be admitted; next concurrent start sees the reservation. Unknown confidence is unchanged. |
| Floor 5%, switch margin 3 points | At observed 8%, new independent and soft-affinity work uses an eligible peer with no floor or above its own switch point when one exists, but a hard continuation may remain; at observed 5% every new selection stops using this account. Observed 9% has no floor-based switch preference. No floor gives neither threshold. |
| Graceful-switch point crossed during an in-flight request | The already forwarded turn completes. After required refresh writes, a pending switch is checked at its terminal boundary or before the next idle `response.create`; only a live healthy peer triggers reconnect. Without one, the socket remains and new work may fall back to the account while above the floor. |
| Hard floor observed during an active turn | The existing immediate floor signal closes the socket even if the turn is active; later new selection cannot use that account. Work already forwarded may have consumed below the floor. |
| Floor reached with a hard continuation and an eligible peer | The old `previous_response_id` fails explicitly; a fresh conversation can select the peer. The Router never forwards that ID to the peer. |
| Floor raised above saved weekly balance while a WebSocket is open | New selection uses the saved floor immediately; the existing socket is signaled on the next validated refresh that observes the switch point or floor. A frame before that signal may forward. |
| Known failed 5h guard, exhausted weekly quota, disabled account, or hard ownership conflict | Existing failure/exclusion behavior still wins over idle priority. |
| Known weekly runway below 900 seconds | No claim to the unknown-burn one-client exception or the farther-reset known-burn preference. |

Proof must cover pure selector boundaries, actual serialized reservation and affinity selection, quota/status agreement for equivalent inputs, floor-reconnect signaling, machine and human reason formatting, and preservation of existing June/September scenarios except the explicitly superseded cases. Remove or update old tests that assert the deleted retirement behavior; preserve unrelated proof gates.

## Supersession and exclusions

This contract supersedes September S-QA-04 and S-QA-08 only for idle accounts outside 48 hours and the two-stage configured-floor policy, and supersedes June 26 R5's implicit near-zero retirement outcome. The 48-hour drain pool and its accepted inside-horizon sequences otherwise remain. It does not authorize a new global scheduler, persisted probe history, reset-credit routing input, automatic credit redemption, provider API/schema changes, or migration of a hard-owned response continuation.
