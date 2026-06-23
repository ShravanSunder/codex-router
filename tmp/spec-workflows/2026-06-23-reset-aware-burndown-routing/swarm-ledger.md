# Reset-Aware Burn-Down Routing Spec Ledger

Date: 2026-06-23
Status: revised parent synthesis after spec-review findings

## Source Inputs

- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-selection/src/weighted_deficit.rs`
- `crates/codex-router-selection/src/eligibility.rs`
- `crates/codex-router-state/src/quota_snapshot.rs`
- `crates/codex-router-state/src/repositories.rs`
- `crates/codex-router-cli/src/quota.rs`
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- `MEMORY.md:2455-2541` for recovered prior context; treated as secondary to live repo evidence
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/review-ledger.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/*.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r2/review-ledger.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r2/lanes/*.md`

## Lanes Run

| Lane | Agent | Status | Accepted Evidence |
| --- | --- | --- | --- |
| codebase-explorer | Parfit | answered | Accepted: selector reads persisted per-window input but collapses it to minimum headroom before weighted selection. |
| architecture-clean-boundary | Banach | answered | Accepted: burn-down assessment should be pure and shared by selector and CLI; `WeightedDeficitSelector` should stay generic. |
| risk-and-tradeoff-design | Faraday | answered | Accepted: the spec must decide bounded reset salvage and must upgrade selection explanations beyond freshness labels. |
| ux-api-cli-surface | Curie | answered with local evidence gap | Partially accepted: product language and display constraints; local-code claims rejected until parent verification. |
| architecture-minimal-and-pragmatic | Boyle | answered with local evidence gap | Partially accepted: avoid forecasting engine; rejected "earliest reset wins" as too blunt. |

## Parent Verification

Accepted direct observations:

- `account_state_from_selector_input` rejects ineligible windows and reduces multi-window selector data to minimum remaining headroom. Source: `crates/codex-router-proxy/src/account_selection.rs:262-292`.
- `WeightedDeficitSelector` only consumes account id and scalar weight. Source: `crates/codex-router-selection/src/weighted_deficit.rs:66-98`.
- persisted selector windows contain reset time, observed time, window length, status, and remaining headroom. Source: `crates/codex-router-state/src/quota_snapshot.rs:91-200`.
- CLI status already computes pace and runout from reset time. Source: `crates/codex-router-cli/src/quota.rs:924-1007`.
- existing spec already constrains weekly quota protection and selection-visible reset timing. Source: `docs/specs/2026-06-20-codex-router-greenfield-spec.md:147-151`.
- existing plan already identifies long-window pressure ahead of reset urgency. Source: `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md:322-338`.
- spec review found the first draft under-specified across scoring, dependency
  ownership, thresholds, mixed-window collapse, human status output,
  non-blocking proof, and redaction proof. Source:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/review-ledger.md`.
- second spec review found the first revision still under-specified route-band
  batch assessment ownership, unknown fallback semantics, WebSocket routing and
  security order, machine/plain status surfaces, safe-label policy, and security
  proof. Source:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r2/review-ledger.md`.

Rejected or deferred evidence:

- "earliest reset wins" is rejected as the primary policy because it can over-route nearly empty or weekly-dangerous accounts.
- external README/issues from the UX/pragmatic lanes were not accepted as source of truth for this repo. They only support product intuition.
- the dedicated security review lane crashed; the revised spec adds
  surface-by-surface redaction expectations, but the next `spec-review-swarm`
  should rerun a dedicated security/trust-boundary lane.

## Accepted Design Decisions

1. Add a pure burn-down assessment layer over persisted selector windows.
2. Preserve request/startup behavior: no provider quota refresh on startup or request selection.
3. Keep `WeightedDeficitSelector` generic; feed it risk-adjusted weights.
4. Treat long-window pressure as dominant over short-window reset urgency.
5. Allow bounded reset salvage for soon-reset windows only when durable-budget risk is not dangerous, or when the long window itself is imminently resetting.
6. Use structured routing reasons shared by runtime audit and quota status display.
7. Keep default human quota output account-centric and avoid internal score jargon.
8. Put pure assessment in `codex-router-selection::burn_down`; proxy and CLI
   adapt state DTOs into pure assessment DTOs.
9. Use fixed v1 policy constants for near-reset thresholds, reserve thresholds,
   pressure multiplier, salvage caps, and weight clamps.
10. Classify mixed windows with any-window conservative collapse:
    ineligible/exhausted blocks, unknown or missing reset becomes fallback,
    stale marks stale, and `effective` is only an explanation hint.
11. Route by availability pool before weighted fairness:
    `usable`, then `reserve`, then `unknown`, never `blocked`.
12. Make default human status output strict: safe account label only, Unicode
    bars when supported, no `pp`, no `bottleneck`, no raw score, and
    preferred-next explanation when routing is shown.
13. Require black-box non-blocking proof for boot/listen, first routed request,
    and quota status render.
14. Require end-to-end Codex-through-router proof, including WebSocket behavior,
    before implementation completion can be claimed.
15. Make route-band batch assessment the selector-facing contract so one pure
    assessment owns selected pool, weighted candidates, and neutral
    `preferred_next`.
16. Make unknown quota fallback-only; remove the legacy same-pool unknown
    freshness penalty from v1 selection semantics, but preserve conservative
    partial-headroom ordering inside the all-unknown fallback pool.
17. Define `/v1/responses` WebSocket support as a first-class route using the
    `responses` route band, with local auth and first-frame validation before
    selection, credential resolution, or upstream open.
18. Split status surfaces into table/plain human output and explicit JSON
    machine output with stable fields and enums.
19. Use safe account labels or hashes by default in human output, logs, traces,
    smoke transcripts, and selection explanations; raw account id is explicit
    local JSON/debug only.
20. Treat smoke/log/transcript output as allowlisted evidence and forbid raw
    bodies, full WebSocket first frames, prompts, memory traces, tool args,
    unsafe labels, tokens, auth headers, and secret-store material.
21. Define previous-response affinity as a fail-closed continuation contract for
    HTTP/SSE and WebSocket before weighted fallback.
22. Collapse all non-`/v1/responses` WebSocket paths to `unsupported_path`.
23. Require live-safe CLI status smoke over persisted router state for `table`,
    `plain`, and `json`.
24. Require delayed/failing-refresh proof for first valid `/v1/responses`
    WebSocket routing.
25. Define all-unknown fallback as explicit `fallback` next-use output so
    unknown quota never looks healthy while still showing the account the router
    may try when every known pool is empty.
26. Replace prose salvage tiebreaking with an exact salvage tie key shared by
    assessment, status, proxy adapter, and deterministic tests.
27. Forbid fake `0% left` placeholders for unknown, missing-reset, and no-data
    human status slots.
28. Expand JSON status into an audit shape that can reconstruct selected pool,
    next use, displayed window slots, all relevant windows, reset metadata, and
    safe routing explanations.
29. Define previous-response affinity extraction as the top-level
    `previous_response_id` field in HTTP/SSE bodies and first WebSocket
    `response.create` frames, with malformed values failing closed.
30. Require a WebSocket preselection failure matrix covering local auth,
    unsupported path, wrong type, oversized frame, timed-out frame, malformed
    affinity, and owner-resolution failures.
31. Define local Codex-through-router e2e acceptance as installed Codex CLI plus
    generated router profile, served local router, mock upstream, HTTP/SSE and
    WebSocket transport, reset-aware multi-account choice, status agreement,
    pinning, and redacted transcripts. Live OAuth/quota remains separate and
    approval-gated.
32. Treat WebSocket `/v1/responses` route band as path-derived in v1; before
    selection the router may read only top-level `type` and top-level
    `previous_response_id` from the first frame.
33. Split raw quota evidence from final public/audit routing explanation:
    `quota_evidence_reason` records evidence before pool choice, while
    `routing_reason` is assigned after selected-pool mapping.
34. Missing exactly one expected v1 response window, 5h or weekly, makes the
    account `unknown` and renders the missing slot as `no data`.
35. Separate proxy-owned account fact adaptation/runtime enforcement from
    selection-owned pure exclusion/classification. Disabled and
    missing-credential accounts are returned as `excluded` for status, never
    selected.
36. Build burn-down assessments for every supplied route-band account fact row,
    then filter selected pools after classification. `excluded` and `blocked`
    rows remain in `accounts` for status, JSON, logs, and proof but never enter
    `weighted_candidates`.
37. Define previous-response owner records and pin-write semantics: durable
    affinity uses a hashed canonical key, selected account id, credential
    generation, route band, source transport, and creation time; HTTP/SSE and
    WebSocket pin writes may inspect only allowlisted upstream response id
    fields and must not emit raw ids or raw bodies.
38. Make JSON status a normative envelope with top-level route fields,
    `preferred_next_account_id`, `weighted_candidates[]`, `accounts[]`, and
    per-account `window_slots` and `windows`.
39. Split raw local `--format json` stdout from shared artifacts: local JSON may
    expose `account_id`, but logs, traces, smoke transcripts, PR evidence, and
    review artifacts must redact or hash it.
40. Add structural status guardrails: default status is account-centric for the
    user quota route, has one logical row per account with only an optional
    blank-account continuation line, and excludes unrelated route-band rows or
    labels unless a future explicit debug/multi-route mode exists.
41. Define durable previous-response owner lookup keys as full-length lowercase
    hex HMAC-SHA-256 over the domain-separated canonical previous-response key,
    using router-owned secret material. Raw keys are never persisted, helper use
    is centralized before storage/logging/tracing/audit, and duplicate or
    ambiguous owner records fail closed.
42. Hard-cut over affinity storage: existing raw-key rows are discarded or
    ignored during schema replacement, and no raw-key fallback remains.
43. Map previous-response owner route eligibility to burn-down availability:
    `usable` and `reserve` owners are valid; `unknown`, `blocked`, and
    `excluded` owners fail closed before weighted fallback.
44. Define `router_affinity_hash_secret` lifecycle: generated once per router
    root, persisted independently from bearer/account credential rotation,
    stable across restart and refresh, and non-rotating in v1. If it is missing,
    unreadable, or replaced, existing owner rows are ignored or purged and
    continuations fail closed.
45. Define concrete previous-response affinity boundaries: core owns typed
    affinity/HMAC helpers, secret-store owns `router_affinity_hash_secret`,
    state stores only hashed owner records, and proxy owns edge extraction,
    secret loading, lookup/write orchestration, and fail-closed enforcement.
46. Define affinity repository cutover APIs for hashed owner records and forbid
    state repository methods from accepting raw previous-response ids, raw
    canonical affinity keys, request bodies, or response bodies.
47. Add `router_affinity_hash_secret` to security assets, forbidden emission
    surfaces, and proof expectations, including storage identifier and derived
    secret material redaction.
48. Define `affinity_secret_unavailable` fail-closed behavior for
    response-creating HTTP/SSE and WebSocket routes when the hash secret cannot
    be loaded or created.
49. Make `codex-router-selection::burn_down` own the v1 route-band policy
    registry for all currently classified route bands, with unknown route bands
    failing closed before weighted selection.
50. Split deterministic output ordering: `accounts[]` is sorted by
    `account_id`, while `weighted_candidates[]` is sorted by neutral selector
    order.
51. Define deterministic public `routing_reason` precedence for overlapping
    preferred-account explanations.
52. Pin installed-Codex e2e profile local auth to
    `env_key = "CODEX_ROUTER_TOKEN"`, proving installed Codex sends
    `Authorization: Bearer` to the local router for HTTP/SSE and WebSocket.
    Keep `X-Codex-Router-Token` as accepted manual/compatibility ingress and
    reject mismatched mixed-carrier requests.
53. Add `preferred_weekly_reset_soon` so long-window near-reset salvage can be
    explained directly in default status, JSON, runtime audit, and tests.
54. Forbid persisted/shared smoke transcripts from emitting individual raw
    non-allowlisted WebSocket first-frame/body fields such as `model`, `input`,
    `metadata`, `tools`, prompt text, or request body content.
55. Cut previous-response routing away from `codex-router-selection::affinity`
    and raw `AffinityKey`; no v1 previous-response path may import or call that
    old raw-key surface.
56. Define `codex-router-core::routes::RouteBand` as the shared route-band
    identity used by proxy classification, selection policy lookup, and CLI
    status adapters.
57. Normalize route-level assessment output around `route_result`,
    `selected_pool`, `selected_pool_reason`, `preferred_next_account_id`,
    `weighted_candidates`, and `accounts` so supported, unsupported, status,
    runtime audit, and tests consume one shape.
58. Accept installed-Codex direct WebSocket response-create first frames through
    bounded structural booleans while forbidding raw `model`, `input`, `stream`,
    prompt, tool, metadata, or body values from influencing selection or
    appearing in persisted/shared proof artifacts.
59. Define runtime cooldown/pinning as a proxy-owned wrapper over the pure
    assessment: holds and affinity survive only when the account remains in the
    current `weighted_candidates`.
57. Define affinity hash-secret storage as
    `load_or_create_router_affinity_hash_secret`, stable key
    `router_affinity_hash_secret.v1`, 32 random bytes, 64-lowercase-hex
    persisted encoding, typed core return, and redacted errors.
58. Define `SafeAccountLabel` helper semantics, minimum unsafe predicates, and
    `acct-<12 lowercase hex chars>` deterministic redacted tag format.

## Open Decisions

No product decisions remain open before the next spec review. The next review
may still reject decisions, but plan creation must not reopen them silently.

## Next Route

Recommended next skill: `shravan-dev-workflow:spec-review-swarm`.

Only after review acceptance should orchestrator route to
`shravan-dev-workflow:plan-creation-swarm`.

phase_result: complete
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`, `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/swarm-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-review-swarm`
recommended_transition_reason: Revised spec folds in accepted review findings; next hard gate is adversarial spec review before planning.
