# Plan 2: Codex-Safe Exhaustion and Reconnect

Date: 2026-06-26
Status: reviewed once; accepted findings addressed
Source spec: `docs/specs/2026-06-26-quota-routing-safety-spec.md`
Depends on: `docs/plans/2026-06-26-quota-routing-plan-1-sqlx-strict-routing.md`

## Deliverable

After this plan, Codex does not see one account's quota/auth exhaustion while
another configured account can serve. The router retries or reconnects only at
safe boundaries, using behavior proven against Codex source and installed Codex
runtime behavior.

This plan keeps the router law intact: account routing and OAuth/quota safety
belong to codex-router; prompt/tool/message semantics and payload protocol
remain Codex-owned.

## Requirements covered

- R4 Codex-safe account exhaustion
- R5 near-zero retirement
- R6 affinity and hold semantics under exhaustion
- R8 slice-local exhaustion/reconnect telemetry
- R10 threat model controls for provider errors and payload boundaries
- R11 WebSocket/pass-through proof gates

## Work

1. Reconnect evidence-first gate
   - Before implementing account-switch behavior that depends on it, prove the
     selected signal makes installed Codex reconnect through the router and
     complete the turn.
   - Current candidate: in-band Responses WebSocket error frame with
     `websocket_connection_limit_reached`.
   - Do not use WebSocket close reason text.
   - Do not use `usage_limit_reached` as the router switching mechanism.

2. Exhaustion state and retry ledger
   - Persist `suspect_exhausted` with route-band scope.
   - Make `suspect_exhausted` non-selectable for new work, affinity reuse, and
     hold reuse until refresh/reset/probe/TTL clears it.
   - Keep a per-request or per-connection attempted-account ledger.
   - Cap retry/reconnect attempts so a bad account cannot loop forever.

3. Downstream commit boundary
   - Retry/reselect invisibly only before provider bytes are committed to Codex.
   - After HTTP/SSE bytes are committed, do not add broad buffering or arbitrary
     payload interpretation.
   - If a provider failure can be safely classified after commit, update quota
     state only for future requests.

4. Near-zero retirement
   - Initial policy:
     - retire from new work when projected runout is under 30 minutes;
     - retire from new work when the limiting window is under 5%;
     - do not retire if all alternatives are worse or unavailable;
     - treat reset-soon accounts as usable only when projected runout survives
       to reset with configured safety margin;
     - preserve same-turn affinity unless Codex reconnect proof authorizes a
       safe transfer.
   - Respect same-turn Codex sticky-state semantics.
   - Keep same-turn/previous-response affinity only when the account is still
     usable or reserve-usable.

5. Pass-through guard tests
   - Add protocol canaries with unknown fields and payload fragments.
   - Prove the router does not validate, rewrite, or log Codex payloads beyond
     route/auth/affinity/quota-retire boundaries.

6. Slice-local observability
   - Emit exhaustion, suspect mark, attempted-account, retry-cap,
     near-zero-retire, reconnect, and all-accounts-exhausted telemetry for the
     Plan 2 paths.
   - Use scrubbed account slots/hashes only.
   - Prove negative redaction canaries for provider bodies, account labels, raw
     ids, prompts, paths, tokens, and reservation ids in exhaustion/reconnect
     events.

## Likely files

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/http_sse.rs`
- `crates/codex-router-proxy/src/provider_error.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-state/src/*`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `tests/smoke/installed_codex_mock.sh`

## TDD gates

Red first:

- installed Codex reconnect proof fails until the router emits the exact accepted
  reconnect signal through the real router path;
- `usage_limit_reached` is not accepted as hidden account-rotation behavior;
- `suspect_exhausted` blocks new work, affinity reuse, and hold reuse;
- retry cap fails if the same bad account is attempted more than once for the
  same exhaustion path;
- pass-through canary fails if the router validates, rewrites, buffers broadly,
  or logs payload fragments;
- near-zero threshold tests fail until the initial policy covers new-turn
  retirement, same-turn affinity preservation, reset-soon behavior, and
  all-alternatives-worse fallback.

Green proof:

- WebSocket test: first account emits accepted exhaustion/reconnect signal,
  second account completes, Codex sees a successful turn;
- all-accounts-exhausted test: Codex receives a router-level exhausted error
  only when no configured account can serve;
- HTTP/SSE pre-commit test: first account quota response is hidden when a second
  account can serve;
- HTTP/SSE post-commit test: router does not invent broad payload buffering and
  only updates future state when safely classifiable;
- same-turn affinity test: no unsafe account switch without proven reconnect;
- telemetry proof for exhaustion/reconnect/retire/all-accounts-exhausted events;
- negative telemetry canary for provider-error and reconnect paths.

## Validation commands

Exact test names may change during implementation, but the plan must prove:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p codex-router-proxy --lib -- websocket`
  - proves R4 WebSocket reconnect/exhaustion and pass-through behavior
- `cargo test -p codex-router-proxy --lib -- http_sse`
  - proves R4 downstream commit boundary for HTTP/SSE
- `cargo test -p codex-router-proxy --lib -- provider_error`
  - proves R4 classifier distinctions and no `usage_limit_reached` rotation
- `cargo test -p codex-router-selection --lib -- near_zero`
  - proves R5 near-zero policy edge cases
- `tests/smoke/quota_routing_plan2_observability.sh`
  - proves R8/R10 for exhaustion/reconnect paths and redaction canaries
- installed-Codex mock smoke with three concurrent WebSocket clients
- `git diff --check`

If validation needs router isolation from the user's default port, first add or
verify an explicit non-default-port harness flag before using that as proof.

## Stop conditions

- Stop if installed Codex does not reconnect and complete with the chosen signal.
- Stop if the only working solution requires broad payload parsing/validation.
- Stop if all-account exhaustion can leak one account's provider body.
- Stop if exhaustion/reconnect paths cannot be observed without raw account or
  provider-body leakage.

## Checkpoint commit

Commit after Plan 2 passes all gates. This commit should not include Plan 3
observability work beyond minimal traces needed to debug Plan 2 behavior.
