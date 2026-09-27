# Idle quota drain and floor protection requirements

Date: 2026-09-25
Identity: `idle-quota-drain-requirements-2026-09-25`

## Purpose

Developers using Codex Router should be able to spend an idle account's remaining weekly allowance through real new work, even when its natural reset is more than 48 hours away. A configured weekly floor is the operator's hard reserve: the Router starts moving work away three percentage points above it and stops using that account at the floor. Existing conversations remain on their account while eligible. The Router does not create traffic to consume quota or redeem reset credits automatically.

This change addresses the observed case where an account with 2% remaining and no clients is shown as retiring, and another with 7% remaining and no clients is available but not preferred because its reset is 53 hours away. One existing client remains on a different account. The screenshot and current source establish the behavior; the exact live selector inputs may differ from the status mirror at a particular instant.

## Governing baseline

The [2026-09-18 quota allocation requirements](../2026-09-18-quota-allocation/2026-09-18-quota-allocation-requirements.md), [specification](../2026-09-18-quota-allocation/2026-09-18-quota-allocation-specification.md), and [program design](../2026-09-18-quota-allocation/2026-09-18-quota-allocation-program-design.md) remain authoritative except for the bounded policy changes below. The June 26 near-zero retirement rule and the June 27/28 scenario outcomes are superseded only where these requirements expressly disagree. Existing hard response ownership, eligible soft affinity, hard quota blocks, reported short-window guards, weekly runway safeguards, and truthful quota evidence remain obligations.

## Authorized outcomes

| ID | Need and observable outcome | Authority |
| --- | --- | --- |
| U-ID-01 | New independent work may use an idle account with fresh, positive weekly allowance above its configured floor even when its reset is more than 48 hours away and no reset credit is available. The Router prefers eligible expiring idle allowance ahead of an already busy far-reset account, while retaining the existing near-reset drain priority. | Owner's 2026-09-25 direction to use the zero-client 2% and 7% balances and allow early exhaustion without a credit. |
| U-ID-02 | An idle account with unknown or zero measured burn may receive one real client at a time so real use can establish a burn estimate. Its uncertainty stays visible; a reservation prevents concurrent initial assignments from all treating it as idle. | Owner's explicit answer that a client is needed to obtain burn data. |
| U-ID-03 | No implicit 5% remaining threshold or 30-minute projected-runout retirement excludes an otherwise eligible account from new work. An explicit configured floor and existing independent quota/flow guards still apply. | Owner's explicit removal of both near-zero retirement triggers. |
| U-ID-04 | With a configured weekly floor `F`, begin a best-effort graceful switch at observed `F + 3` percentage points: send new independent work and eligible soft-affinity work to another healthy account when one exists, while a hard-owned response continuation may finish on its owner above `F`. After a validated quota observation and its burn history have been saved, let an established WebSocket's active turn finish, then prompt it to reconnect before forwarding further work only when the live Router can route a healthy peer; leave it connected if no peer exists above `F`. At or below observed `F`, stop using that account for every request entering selection and promptly reconnect its WebSockets regardless of peer availability; new work uses another eligible account, while a hard-owned `previous_response_id` cannot be silently transferred and must restart as a fresh conversation. With no configured floor, neither threshold applies. In-flight work and frames racing an observation may cross `F`; the margin does not guarantee an exact provider balance. | Owner's 2026-09-26 correction: `F + X` begins a graceful switch, `F` is the new zero and another account must carry subsequent work. The previously chosen fixed `X = 3` points remains the gap. Owner confirmed active-turn completion before the early switch. Existing provider response ownership forbids transferring the same continuation ID to another account. |
| U-ID-05 | The quota/status surface explains why an idle account is preferred or held and shows both the graceful-switch point and the hard floor. It never reports a retired state after that policy is removed; degraded load authority remains labeled. | Required for operators to understand U-ID-01 through U-ID-04 from the same shared assessment used by runtime. |
| U-ID-06 | Existing conversation ownership, hard provider exclusions, short-window safety, and reset-credit redemption remain separate from idle-drain ranking. No account is moved merely to consume quota, and no credit is consumed by routing. | Preserved September allocation and July guarded-reset contracts; owner chose early use without a credit, not automatic redemption. |
| U-ID-07 | While Router's background quota observation is enabled, begin a new observation cycle on a three-minute cadence so floor and routing decisions receive fresher provider evidence. A slow or failed provider call remains visible rather than being represented as a fresh observation. | Owner's 2026-09-25 request to observe quota every three minutes. |

The owner corrected the threshold meaning on 2026-09-26: the three-point margin begins a graceful switch, and the configured floor itself is the hard stop. The margin is not a provider guarantee.

## Scope and limits

In scope: new-assignment ranking of eligible idle accounts, removal of the two near-zero retirement triggers and now-unused status states, extension of one-client initial admission beyond 48 hours, configured-floor graceful-switch and hard-stop thresholds, a three-minute background quota cycle, status explanation, and fitting tests. The current 48-hour near-reset pool remains a priority for accounts that meet its evidence and runway requirements.

Out of scope: moving a usable existing conversation to spend quota, synthetic probe requests, credit-aware routing, automatic credit redemption, changing account credentials or provider APIs, changing the configured floor's 1–15% input range, global scheduling, production process replacement, merge, and release.

## Success boundary

For fresh, eligible, unowned new work, a zero-client account above its configured floor can receive work despite 2% remaining or a reset beyond 48 hours; inside the three-point switch band a healthy peer takes that work when available. A known failed guard, hard exhaustion, the configured floor, or a hard response owner still governs its own case. A first unknown-burn reservation is visible to the next selection. At the floor stop, a hard continuation entering selection refuses rather than silently selecting another account, so the client must start fresh to use a peer. A validated refresh makes an already established WebSocket eligible for a graceful switch after saving burn history and selector window; its active turn completes before a reconnect is sent, and the live Router must confirm a healthy peer before interrupting further work. At the hard floor the existing reconnect signal is prompt and unconditional. Runtime and status use the same assessment for equivalent fresh inputs, with status load-mirror lag described honestly.
