# codex-router Quota Routing Safety Plan Index

Date: 2026-06-26
Status: split into vertical implementation plans
Source spec: `docs/specs/2026-06-26-quota-routing-safety-spec.md`

## Goal

Deliver the quota-routing safety spec through smaller vertical plans that each
produce a usable, testable product improvement. The spec is one contract; the
implementation is split so each plan can be reviewed, implemented, proven, and
checkpointed independently.

## Plan set

1. `2026-06-26-quota-routing-plan-1-sqlx-strict-routing.md`
   - Deliverable: codex-router-owned storage is SQLx-only, and the runtime no
     longer routes new non-affinity work to weak accounts through smooth weighted
     deficit.
   - User-visible proof: `codex-router quota` agrees with runtime selection for
     cached quota state and active-load pressure.

2. `2026-06-26-quota-routing-plan-2-codex-safe-exhaustion.md`
   - Deliverable: one account's quota/auth exhaustion does not reach Codex while
     another configured account can serve, without adding payload validation or
     protocol invention.
   - User-visible proof: installed Codex reconnect/retry behavior is proven
     through the router, including three concurrent WebSocket clients.

3. `2026-06-26-quota-routing-plan-3-observability-live-proof.md`
   - Deliverable: cross-slice live proof hardening for the telemetry already
     added by Plans 1 and 2. This is not the first observability work.
   - User-visible proof: fresh marker queries show the full routing/reconnect
     path and negative canary queries prove sensitive data is absent.

## Dependency order

```text
Plan 1: SQLx strict routing + quota truth
  |
  v
Plan 2: Codex-safe exhaustion + reconnect
  |
  v
Plan 3: OTEL/Victoria proof + final live validation
```

Plan 2 may begin only after Plan 1 proves the selector and storage boundaries.
Plan 1 must prove strict-selection telemetry locally. Plan 2 must prove
exhaustion/reconnect telemetry locally. Plan 3 is reserved for cross-slice live
soak/query proof after those local gates pass.

## Shared gates

Every plan must run:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- the plan-specific unit/integration/smoke gates named in that plan
- `git diff --check`

Each plan must be reviewed before implementation if materially changed. The
implementation for each plan must receive implementation review before merge.

## Non-goals across all plans

- No Codex CLI changes.
- No disabling WebSockets.
- No per-message WebSocket account switching.
- No broad CLI cleanup outside quota/status behavior.
- No second sqlite Rust client library; SQLx only for direct sqlite access.
- No Codex payload validation beyond route/auth/affinity/quota-safety
  boundaries explicitly allowed by the spec.
