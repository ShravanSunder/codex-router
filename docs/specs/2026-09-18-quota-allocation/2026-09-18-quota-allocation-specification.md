# Quota Allocation Specification

Date: 2026-09-18; revised 2026-09-20
Identity: `quota-allocation-specification-2026-09-18`
Requirements: [Quota Allocation Requirements](2026-09-18-quota-allocation-requirements.md)

## Domain entities

| Identity / canonical term | Same-instance rule and relationships | Invariant and observable states | Source terms / authority |
| --- | --- | --- | --- |
| E-QA-01 Account | One configured provider account; credentials can refresh without creating a different account. One account serves many sessions and has zero or more reported windows. | Eligibility is assessed from current evidence; enabled/disabled, eligible/held/blocked are distinct from identity. | OAuth account; U-QA-01/04/05/06 |
| E-QA-02 Session | One supplied session identity, independent of transport connection or reconnect. It has at most one current soft-affinity owner. | May be unassigned, freshly assigned, expired, or reassigned. Expired affinity does not end the session. | Conversation, session; U-QA-01/03/07 |
| E-QA-03 Soft affinity | The current session-to-account assignment. A replacement assignment supersedes the prior ownership instance, even when a later assignment returns to the same account. | Fresh below two hours idle; expired at equality. Old ownership activity cannot renew its replacement. | Stickiness; U-QA-03/07/08 |
| E-QA-04 Hard response ownership | Ownership of one prior provider response. Many responses may belong to one account. | Resolved, missing, ambiguous or unavailable ownership determines continuation behavior; soft affinity cannot substitute for it. | Previous-response owner; U-QA-02 |
| E-QA-05 Quota window | A reported allowance for one account, route band and reset period. Weekly and short-term limits are distinct windows; a new reset period is distinct from the old one. | Fresh, stale, unknown, eligible or exhausted evidence remains distinguishable; an omitted limit after successful refresh is absent. | Weekly/short-term quota; U-QA-04/05/06/10 |
| E-QA-06 Active client | One currently held routing reservation in a route band, assigned to one account; later real work after release starts a new active interval. | Acquired to active to released/retired. An affinity row or an idle socket alone does not establish activity. | Connection/load; U-QA-05/11 |
| E-QA-07 History record | One recorded occurrence within its history class. Different classes retain their existing identities and cutoff clocks. | Current/protected, retained, then deletion-eligible and deleted under S-QA-09. Open activity remains protected. | Usage/log history; U-QA-12 |

Session ownership (E-QA-02/03/04) controls S-QA-01/02/03/05/06. Account, quota and active-client evidence (E-QA-01/05/06) controls S-QA-04/07/08/10. History records (E-QA-07) follow S-QA-09.

## Observable contract

This Specification preserves the accepted June 26–28 contracts except for the two-hour affinity duration, the explicit idle-account admission rule, and the S3m fixture correction below. It makes weekly-only quota support explicit and bounds retention repairs to existing maintenance owners.

```text
Codex developer -- real requests --> Router -- authenticated requests --> Provider
Codex developer <-- result/error -- Router <-- responses/quota evidence -- Provider
Operator        -- quota/status --> Router -- scrubbed status --> Operator
```

No provider requests are generated to drain quota, renew affinity, or manufacture a burn estimate.

### S-QA-01 — Hard continuation ownership

On supported routes, a resolved `previous_response_id` owner takes precedence over soft affinity and ordinary quota preference. Missing, ambiguous, excluded, or unavailable ownership never silently selects another account. Existing admission, quota and state errors may terminate processing before ownership lookup; their precedence remains unchanged. A found owner must still pass account eligibility checks.

### S-QA-02 — Soft affinity

When no hard owner applies and a session identifier is present, the Router prefers a soft-affinity owner while the account is `Usable`, `Reserve`, or `Retiring` and the idle age is strictly less than 7,200 seconds. Missing, expired, excluded, or unavailable soft affinity may fall through to new-assignment selection.

### S-QA-03 — Sliding boundary and activity

Idle age less than 7,200 seconds is eligible. Exactly 7,200 seconds and any greater age are expired. Real request activity renews the session's last-seen time. Timers, pings, Pong, close/control frames, synthetic keepalives, and background model calls do not renew it. Router affinity duration makes no provider cache-retention claim.

### S-QA-04 — New assignment

When no hard owner or eligible soft affinity applies, apply S-QA-08's initial-admission exception, then the accepted quota policy. Candidate evaluation includes current active sessions plus one projected connection, burn evidence with its confidence, all reported windows, reset timing, hard blocks, applicable short-term safety, weekly margins, and controlled-drain rules.

Weekly resets within 172,800 seconds, inclusive, qualify for the existing near-reset preference when its evidence and guard rules hold. Existing client load is included; controlled drain remains permitted after drain gaps become non-positive. Preserve the 900-second floor for calculable weekly runway, the 200-basis-point weekly margin, and the 100-basis-point short-window margin. S-QA-08 alone admits an initial client without a calculable runway. No one-hour rule, multiplier, twenty-percent cutoff, project budget, or unrelated comparator change is introduced.

For calculable short-term consumption, the normal guard requires remaining quota minus consumption predicted until reset to be at least 100 basis points (one percentage point). Where only exhaustion time is available, retain the existing 30-minute near-reset fallback and last-resort behavior. Missing estimates are not proof of safety; a known failed short-term guard cannot be bypassed by S-QA-08.

### S-QA-05 — Concurrent first selection

For concurrent first selections with the same previously unseen session identifier, the selected eligible owner becomes visible before asynchronous durable persistence completes, provided hard ownership, exclusions, and eligibility remain unchanged. Persistence latency or failure does not block forwarding. A later stronger owner or eligibility change may supersede it.

### S-QA-06 — WebSocket renewal

For an established WebSocket, soft affinity renews after a text message recognized by the existing bounded top-level metadata classifier as type `response.create` has been forwarded successfully. The original message remains pass-through, including large prompts. Renewal does not require full-message JSON validation or provider acceptance: recognized request metadata may renew affinity even when other content is malformed. Messages not recognized as `response.create`, control traffic, and failed sends do not renew. A stale socket cannot renew an owner superseded by another selection, including an A-to-B-to-A cycle.

## S-QA-10 — Observable surfaces and proof

The quota/status surface exposes deterministic next-account reasoning, limiting window, projected runout, active-load evidence, confidence, freshness, and scrubbed failures. Stale or unavailable load is labeled. Accepted pre-commit retry, post-commit pass-through, bounded provider-error handling, all-accounts-exhausted behavior, SQLx-only state, telemetry scrubbing, and protocol pass-through remain unchanged.

Proof must cover the June 28 scenarios with the explicit S3m correction and S3k expansion below; hard-owner precedence; 7,199/7,200/7,201 boundaries; recognized request activity for small and large messages, including malformed content after recognized metadata; non-renewing unrecognized/control traffic; concurrent first selection with delayed persistence; failed persistence; successful and failed WebSocket forwarding; stale handles; A-to-B-to-A; and operator/runtime agreement.

## S-QA-07 — Reported windows and burn evidence

A successful refresh with valid weekly quota and no short-term window replaces the previous window set; obsolete short-term rows cannot survive replacement or block a weekly-only account. Apply short-term guards only to reported limits. Failed refreshes do not prove a limit disappeared and retain existing failure/staleness behavior.

For normal forecasts, use observations within the same reset period. The latest observation must be at most 300 seconds old. Three or more observations spanning at least 900 seconds give Normal confidence; a usable slope over a shorter positive interval or from two observations gives Low confidence. One observation is Insufficient; absent observations or reset time are Unknown; older observations are Stale. Missing active-client history falls back to aggregate burn, lowering Normal confidence to Low and preserving other confidence labels. Zero measured burn provides no finite exhaustion forecast, not guaranteed infinite capacity. Preserve these labels during initial admission.

## S-QA-08 — Initial real work for idle near-reset accounts

An account qualifies for initial admission only when:

- it is enabled, has usable credentials, is not excluded for this attempt and has no hard usage-limit block;
- weekly quota is fresh and Eligible, with positive remaining allowance above any configured floor, and reset is in the future and at most 172,800 seconds away;
- each reported short-term window has fresh Eligible positive quota; a known failed short-term guard disqualifies it. When neither consumption nor exhaustion can be calculated, the absence of a guard estimate does not disqualify this one-client exception. Do not label that unknown estimate as a passing forecast;
- authoritative active-client count for this route band is zero; and
- weekly runway is unavailable because burn evidence is missing, insufficient, stale, or zero. A known positive-burn runway below 900 seconds cannot use this exception.

For this initial-admission exception, the configured weekly floor remains binding and the built-in below-5% retirement hold does not apply. The ordinary reserve and retirement heuristics cannot remove a qualifying candidate. This supersedes June 26 R5 only for S-QA-08; hard blocks, exhaustion and known failed guards remain binding. A qualifying candidate is reported Usable with the initial-admission reason when selected; unrelated accounts retain their ordinary availability.

For new unowned work, prefer these candidates over ordinary burn-ranked accounts. Rank multiple qualifying idle accounts by earlier weekly reset, then higher weekly remaining quota, then stable account identity. This narrow initial-admission order does not replace the normal comparator.

Assign one real client and include its reservation before a subsequent selection evaluates idle status. Concurrent starts must not all see zero and pile onto one account. While the client is active, the account no longer receives the initial-admission preference; ordinary policy governs additional clients. If the account becomes idle again, reevaluate using current evidence. An affinity row alone is not active consumption. No synthetic requests or migration of existing sticky conversations occurs. Complete draining depends on sufficient real user demand.

Expose a distinct scrubbed selection reason, preserving actual confidence and unknown runway. Runtime and quota/status selection must agree for equivalent fresh inputs. Status carries the existing active-load source and freshness labels; a lagging database mirror is not equivalent to the runtime's current load and may temporarily show a different next candidate.

### Scenario supersession and boundary cases

These refinements distinguish quota freshness from burn confidence. A fresh quota snapshot with an unknown burn estimate is not unknown quota. Unless stated otherwise, initial-admission fixtures have fresh Eligible weekly and short windows, usable credentials, no exclusions or configured floor, and zero active clients. Requests are new, unowned work.

| Governing scenario or new case | Prior outcome | Required outcome |
| --- | --- | --- |
| June 27 W4: A 20%, reset 24h, unknown burn; B known, reset 96h | B first | A first with initial-admission reason; while A remains active, this exception no longer applies to A |
| June 27 W8: A 21%, reset 24h, unknown burn; B known, reset 72h, three active | B first | A first with initial-admission reason |
| June 28 S2d, fresh-quota/unknown-burn interpretation: A reset 24h, B known reset 5d | B, final A=0/B=1 | A, final A=1/B=0; unknown-quota variant still selects B |
| June 28 S3f as implemented: B has Stale weekly and short quota | A,A,A,A,A | Unchanged; stale quota fails admission |
| S3f fresh-quota/stale-burn variant: B 42%, reset 25h, no runway; A 40%, reset 24h, known, two active; C known, reset 5d | Known A preferred over B | B,A,A,A,A with unchanged evidence and no releases; B loses the exception after its first reservation |
| June 28 S1f with stale quota | A with stale-only reason | Unchanged; no initial-admission reason |
| S1f fresh-quota/stale-burn variant, reset 24h, no runway | Ordinary single-account fallback | A with initial-admission reason; actual burn confidence remains Stale |
| June 27 U3 / June 28 S1e: unknown quota | Known peer wins, or existing unknown fallback when alone | Unchanged; no initial-admission reason |
| June 28 S1g: zero burn, three active clients | A, fourth active client | Unchanged; nonzero load excludes initial admission |
| Weekly-only A: 20%, reset 24h, missing burn; B known reset 5d | Ordinary ranking | A receives one initial client; no short-window guard is fabricated |
| Reported short window: A weekly 20%, reset 24h; short 20%, reset 4h; no burn estimates | Ordinary ranking | A qualifies for one initial client; unknown runway stays unknown |
| Same case, short burn 6%/h projected after admission | A fails the calculable short guard | A does not qualify: 20% minus 24% fails the 1-percentage-point margin |
| A weekly 4%, reset 20h, no burn, zero active; healthy peer B; configured floor absent or below 4% | A may be Retiring | A receives one initial client; no implicit 5% floor |
| Same A, configured floor 4% or higher | A excluded by floor | Unchanged; A receives no client |
| Positive weekly burn with calculated runway 899 seconds | Existing ordinary policy | No initial-admission exception, even when idle |
| Two qualifying accounts A reset 24h and B reset 25h, both 20%, with two concurrent starts and no releases | Ordinary ranking | A then B; the second selection sees A's reservation |

A known calculated runway, a failed guard, stale/unknown quota, an active client, or reset beyond 48 hours retains the previous rule. S4/S5 and known-burn multi-start scenarios retain their accepted expected sequences except for the owner-confirmed S3m correction below. No general unknown-before-known reversal is authorized.

### Known-burn fixture clarification

These are new, unowned assignments with no releases between starts. Hard ownership and eligible soft affinity still take precedence. This section supplies complete example inputs; its burn rates are fixture values, not new policy constants. Use the June 27 default policy, a fixed `now`, the Responses route, fresh Eligible windows, Normal burn confidence, no exclusions, and no configured floor. Every short window has 90% remaining, resets after four hours, and has no failing guard. Derive each weekly candidate burn from per-client burn multiplied by current active clients plus one. Derive both exhaustion time and survival margin from that same candidate burn.

| Scenario | Account | Weekly remaining | Reset after | Burn per client | Initial active |
| --- | --- | --- | --- | --- | --- |
| S3k | A | 30% | 2h | 480 bp/h | 0 |
| S3k | B | 32% | 26h | 100 bp/h | 0 |
| S3k | C | 75% | 5d | 20 bp/h | 0 |
| S3m | A | 18% | 24h | 80 bp/h | 0 |
| S3m | B | 18% | 24h | 40 bp/h | 1 |
| S3m | C | 50% | 5d | 20 bp/h | 0 |

S3k retains the June 28 sequence **A,A,B,A,B**, final active A=3/B=2/C=0. Its earlier “runout after reset” description omitted the rates; the explicit rates above make its margin-crossing example reproducible. The selected weekly runways are 22,500 / 11,250 / 115,200 / 7,500 / 57,600 seconds. A's candidate survival margin falls to 120 bp on the third start, below the existing 200 bp margin, while B can still survive its reset. Later controlled draining remains allowed. Both A and B remain in the near-reset drain pool; C remains outside it and receives no work.

S3m supersedes only the old **B,A,B,A,B** sequence and final A=2/B=4/C=0 with **A,B,B,A,B**, final A=2/B=4/C=0. Initially both candidates have 22.5h projected runway and -120 bp survival margin after the additional client; choosing idle A preserves the existing lower-load tie-break. Subsequent assignments use the unchanged burn-ranked comparator. Selected runways are 81,000 / 81,000 / 54,000 / 40,500 / 40,500 seconds. Each selection is controlled drain; A and B are Usable and C stays outside the drain pool. This correction is authorized by the owner's direction to keep the simpler existing tie-break; no forecast-driven migration is authorized.

For both fixtures, final A and B are Usable and nonpreferred Usable accounts have the existing same-pool reason. S3k final B has the controlled-drain preferred reason. S3m final A has that reason: its 7.5h runway and B's 9h runway fall within the existing two-hour comparability tolerance, so the existing lower-active tie-break favors A. “Far-reset reserve” describes C's role outside the weekly drain pool, not a requirement to change its ordinary Usable availability.

## S-QA-09 — SQLite history lifecycle

Retain existing periods and strict older-than deletion boundaries; exact-cutoff rows remain until a later cleanup.

| Data | Retention and trigger |
| --- | --- |
| Quota observations | Seven days by observation time; completion of existing quota refresh |
| Completed active-session events | Seven days by terminal event; runtime startup/accepted-connection maintenance; preserve unmatched acquisitions |
| Session-load rollups | One day by bucket end; runtime maintenance |
| Soft-affinity rows | Seven days by last-seen; startup then at most once per advancing UTC day on accepted connections |
| Lifecycle journal | Thirty days using persisted retention clock; checkpoint before deletion; startup and hourly maintenance |
| Automation events | Two UTC calendar months by recorded time; existing minute cadence and bounded batches |

Maintenance errors produce a scrubbed failure signal instead of being discarded. A failed lifecycle maintenance iteration leaves the worker running to retry at the next existing tick; it is not reported as successful cleanup. Other jobs retain their existing next-trigger retry behavior, including affinity's next-day retry. No new idle-time purge guarantee, worker, schema, vacuum operation, or retention option is introduced.

Accounts, credentials, previous-response owners, current state, boards/messages, workflow records and instruction revisions receive no new age-based deletion. OpenTelemetry storage and optional file audit logs are excluded. Deletion permits SQLite page reuse; file shrink is not promised.

## Additional proof and traceability

| Contract | Observable evidence |
| --- | --- |
| S-QA-07 | Weekly-only successful refresh removes old short window and routes successfully; failed refresh does not erase limits |
| S-QA-08 | Eligible idle unknown/zero-burn account receives a client; subsequent concurrent assignment sees reservation; stale-quota/blocked/below-floor/known-low-runway candidates fail admission; stickiness unaffected |
| S-QA-09 | Real fixture database deletion preserves cutoff/open/current rows; production maintenance wiring invokes cleanup; injected lifecycle failure leaves later tick operational; failure signal is scrubbed |

Trace: S-QA-01 → U-QA-02; S-QA-02/03 → U-QA-01/03; S-QA-04 → U-QA-04/05/06; S-QA-10 → U-QA-09; S-QA-05 → U-QA-07; S-QA-06 → U-QA-08; S-QA-07 → U-QA-10; S-QA-08 → U-QA-04/05/11; S-QA-09 → U-QA-12. Existing June 28 outcomes remain mandatory outside the specifically identified idle-admission supersession and S3m correction.

## Non-goals

No forecast-driven migration of usable sessions, provider cache lifetime, synthetic keepalive, broad payload inspection, WebSocket message switching, project budget, schema, provider, or production-process change is specified.
