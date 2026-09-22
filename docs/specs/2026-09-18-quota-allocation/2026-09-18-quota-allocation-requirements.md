# Quota Allocation Requirements

Date: 2026-09-18; revised 2026-09-20
Identity: `quota-allocation-requirements-2026-09-18`

## Purpose

Codex Router must use configured weekly quota before it expires while keeping existing conversations on their account so prompt-cache continuity survives. New assignments are where the Router may choose among accounts. Existing usable sessions must not move because another account has a better forecast.

## Authority and preserved baseline

The owner is the decision authority. The accepted quota policy in [the burn-rate specification](../2026-06-27-account-quota-burn-rate-selection.md), its [scenario companion](../2026-06-28-account-selection-tdd-scenario-spec.md), and the applicable [routing-safety specification](../2026-06-26-quota-routing-safety-spec.md) remain authoritative. The current design changes Router soft-affinity retention to two hours and carries forward the two runtime continuity outcomes from the accepted runtime repair contract, restated below as U-QA-07 and U-QA-08.

The two carried outcomes are restated here so this repository is self-contained: concurrent first selections for one session share their selected eligible owner before its asynchronous database write completes (U-QA-07); successful real requests on an established WebSocket renew affinity without allowing superseded sockets to revive an old owner (U-QA-08). The historical contract's 65-minute duration is superseded by U-QA-03.

## Stable requirements

| ID | Need and outcome |
| --- | --- |
| U-QA-01 | A usable existing session remains on its current account for prompt-cache continuity. Quota forecasts do not migrate it. |
| U-QA-02 | A resolved `previous_response_id` owner is stronger than soft affinity and ordinary selection. Missing, ambiguous, excluded, or unavailable hard ownership cannot silently fail over. |
| U-QA-03 | Soft session affinity uses a two-hour sliding Router idle window. Real request activity renews it; synthetic traffic does not. |
| U-QA-04 | New assignments drain accounts whose weekly reset is within the accepted inclusive 48-hour horizon when the accepted safety policy says they can accept work, especially when existing client load would otherwise leave allowance unused. |
| U-QA-05 | New-assignment selection includes current active sessions, projected burn after one additional connection, all reported quota windows, reset timing, burn confidence, and accepted safety gates. Remaining percentage and round robin alone are insufficient. |
| U-QA-06 | Preserve the accepted quota guards, margins, controlled drain and scenario behavior except for the explicitly authorized weekly-only and idle-account admission cases in U-QA-10/11 and the S3m fixture correction below. Apply short-term guards only to reported limits; retain the 15-minute weekly floor when runway can be calculated. |
| U-QA-07 | Concurrent first selections for one session observe one eligible owner before asynchronous SQLite persistence completes; persistence latency or failure does not block forwarding. |
| U-QA-08 | An established WebSocket renews soft affinity after successfully forwarding a request recognized as `response.create`. Reuse lightweight request-type recognition; do not add whole-prompt validation to operate the idle timer. Stale sockets and A-to-B-to-A cycles cannot resurrect an older owner. |
| U-QA-09 | Operators can inspect the selected account, reason, quota window, runout, active-load evidence, confidence, freshness, and scrubbed failure state. |
| U-QA-10 | A valid weekly-only quota response keeps the account routable. An absent short-term window is distinct from a failed refresh, unknown evidence, or exhausted quota. |
| U-QA-11 | Eligible idle accounts resetting within 48 hours receive new unassigned work instead of being indefinitely overlooked because burn history is missing or zero. Existing conversations stay sticky; draining uses actual user demand. |
| U-QA-12 | Router-owned SQLite history expires under its existing table-specific policies while active work and durable state survive. Cleanup failures are visible and a failed lifecycle iteration does not permanently stop periodic maintenance. OpenTelemetry storage is excluded. |

All rows are authorized: U-QA-01/03/04/05 express the owner's continuity and quota-utilization decisions; U-QA-02/06/07/08/09 preserve the accepted sources above; U-QA-10/11 reflect the September 20 weekly-only and idle-draining clarification; U-QA-12 reflects the SQLite retention request and bounded reliability follow-through. The owner calls stickiness paramount and quota utilization required. Other rows are required supporting behavior, with no new numeric priority scheme.

For U-QA-03/08, the owner clarified on September 20 that the two-hour timer and renewal rule should stay simple and minimize extra logic. Request recognition and successful forwarding establish renewal; full-message JSON validity and provider acceptance are not prerequisites. This includes large prompts and accepts renewal when recognized request metadata precedes malformed content.

For U-QA-11, the owner explicitly permits one initial real client when a reported short-term window is fresh and positive but has no calculable burn estimate; known failed guards still disqualify it. This accepts uncertainty about that client's uninterrupted duration rather than inventing a safe runway. The owner also authorizes this one-client exception below the built-in 5% retirement threshold: only the configured floor applies as a remaining-quota floor. Exhaustion, hard blocks and known failed guards remain binding.

Developer journey: start work without stranding allowance (U-QA-04/05/10/11), continue without unnecessary account changes (U-QA-01/02/03/07/08), and understand limit failures (U-QA-06/09). Operator journey: inspect history retention and recover storage without deleting active work (U-QA-12).

```text
Developer
  existing conversation -> preserve account -> reuse cached prompt context
  new work -> use eligible expiring quota -> avoid stranded allowance
Operator
  inspect quota/load/reasons -> understand assignments
  inspect retained history -> keep useful evidence without unbounded old data
```

## Scope

In scope: new-assignment allocation including initial admission for idle accounts, weekly-only quota, ownership precedence, two-hour affinity, activity renewal, burn projection, reset-aware draining, and bounded SQLite maintenance reliability. Retention periods and cleanup triggers remain existing policy.

Out of scope: forecast-driven migration, a one-hour rule, a ten-percent cushion, a general twenty-percent cutoff, unrelated comparator changes, project budgets, provider cache claims, synthetic keepalive, provider/schema changes, vacuuming, global retention changes, new cleanup schedulers, and production replacement.

## Success boundary

June 28 scenarios remain authoritative outside U-QA-11's initial-admission exception and the owner-confirmed S3m correction: for new unowned assignments with equal reset and projected runway after adding a client, preserve the existing lower-active-client tie-break. This corrects the old busy-account-first expectation; it does not change the ordinary ranking algorithm or override eligible stickiness. A fixture meeting that exception must explicitly record the superseded outcome, not silently weaken its assertions. Affinity is eligible only at idle age `< 7,200` seconds. Unknown burn remains unknown even when one client is admitted. Hard limits remain enforceable and persistence remains asynchronous.
