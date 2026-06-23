# R14 Spec-Difference + Validation Lane

Date: 2026-06-23
Reviewer lane: Socrates (`019ef5f4-bc9a-7903-a5dd-c4b53fde6ca8`)
Spec: `../reset-aware-burndown-routing-spec.md`
Parent baseline: `f104ff9`

## Coverage Receipt

Reviewed the spec against current implementation behavior and current tests,
focusing on places where the code already embodies a different decision than
the spec.

Inspected current paths:

- `crates/codex-router-selection/src/burn_down.rs`
- `crates/codex-router-selection/src/lib.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-test-support/src/installed_codex.rs`
- `crates/codex-router-state/src`

## Verdict

needs_revision

The spec is substantially ahead of the code in several places. That is allowed,
but some gaps are not represented as explicit cutovers or proof rows, so a plan
would not know which behavior to delete, preserve, or reinterpret.

## Accepted Findings

### BLOCKER: Unknown-Quota Fallback Pool Conflicts With Current Selection Semantics

The spec requires unknown quota to become a fallback pool that can be used only
after probing or when no known usable account exists. Current selection code
models unknown as `ProbeRequired`, and the selected pool currently has usable,
reserve, or none outcomes rather than an explicit fallback pool.

Refinement needed:

- Define `fallback` as a first-class selected pool if it remains required.
- Define probe result transitions: usable, blocked/no-auth, quota-empty, or
  still-unknown.
- Add deterministic ordering and cooldown/pinning behavior for fallback use.
- Add tests that prove all-unknown state does not silently become "no route"
  unless probing has failed.

### BLOCKER: Route-Band Policy Ownership Differs Between Spec and Code

The spec requires selection-owned `RouteBand` and policy lookup, including an
`unsupported_route_band` route result. Current code passes caller-owned strings
and caller-injectable policy into the burn-down assessment.

Refinement needed:

- Define a core `RouteBand` enum or equivalent typed contract.
- Remove caller-injected policy from the normal selection API.
- Define the unsupported-band result in the canonical route-level envelope.
- Add tests that prove callers cannot silently override routing policy.

### BLOCKER: Generated Codex Profile Proof Still Targets `env_key`

The current CLI tests still assert generated Codex profiles use `env_key`.
The spec requires the opposite.

Refinement needed:

- Reconcile the generated-profile target with installed Codex compatibility.
- Update proof rows to include the exact CLI profile output and installed Codex
  e2e path.

### IMPORTANT: Status and JSON Surfaces Still Reflect Older DTO Ownership

Current CLI code computes display fields such as routing and next-use text from
older DTOs. The spec wants shared assessment output to own routing semantics,
weighted candidates, route result, and window slots.

Refinement needed:

- Define the exact DTO shape owned by selection.
- Define which fields CLI may format versus which fields it must only render.
- Add JSON golden tests for the machine-readable shape.

### IMPORTANT: Refresh Persistence Contract Is Ahead of Current State Schema

The spec introduces durable refresh status, but current state code persists
snapshots and selector windows without the named `quota_refresh_status` shape.

Refinement needed:

- Add schema and repository ownership to the spec.
- Define stale, refreshing, success, failure, and next-at transitions.
- Add startup tests that prove no synchronous quota refresh is required before
serving requests.

## What Held

- Unknown quota must not be treated as healthy.
- Failed/no-auth/quota-empty states must block routing until a background probe
  says otherwise.
- Cooldown and pinning need to interact with fallback probing.

## Recommended Route

Return to `shravan-dev-workflow:spec-creation-swarm` before plan creation.
