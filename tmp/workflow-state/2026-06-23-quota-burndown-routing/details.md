# Goal Details: 2026-06-23-quota-burndown-routing

## Objective

Deliver reset-aware quota burn-down routing and quota status for `codex-router`
as a fully proven product path, not only a spec, plan, or partial
implementation.

Completion requires:

- accepted revised spec
- accepted implementation plan that traces every task back to the goal and spec
- adversarial plan review passed or findings folded back into the plan
- implementation completed for the accepted plan
- implementation review findings addressed or explicitly rejected with evidence
- full proof loop captured, including end-to-end Codex-through-router behavior
- PR created or updated and proven ready, but not merged unless separately
  authorized

## Scope

In scope:

- reset-aware quota burn-down algorithm
- account classification and routing decisions across 5h and weekly windows
- shared quota assessment semantics for runtime routing and status display
- human quota/status UX with concise account-centric rows, Unicode bars, and
  explicit preferred-next explanation
- non-blocking startup and request behavior using persisted quota state
- background refresh behavior and stale/unknown/ineligible handling
- proof gates across unit, integration, smoke, and end-to-end runtime paths

Out of scope unless explicitly brought back into this goal:

- merging the PR
- unrelated OAuth/keychain work not required by quota routing/status proof
- destructive cleanup of unrelated dirty worktree files
- weakening or deleting proof gates to make the lifecycle pass

## Required Reading

- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/review-ledger.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/algorithm-prior-art-crux.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/contract-architecture-difference.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/planning-adversarial-crux.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/requirements-validation.md`
- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23/lanes/ux-progressive-guardrails.md`

## Accepted Spec Review Findings

The current spec is not accepted. The review found these required fixes:

1. Make the burn-down score to selector weight contract normative.
2. Define shared ownership and dependency edges for assessment DTOs, selection,
   proxy adapters, state DTOs, and CLI display.
3. Freeze threshold and reset-salvage policy as fixed v1 constants or named
   config defaults with rationale and proof bounds.
4. Define mixed window collapse for ineligible, stale, unknown, missing reset,
   no effective row, and empty window set.
5. Make human quota/status output strict: at most two rows per account, Unicode
   bars, no `pp`, no `bottleneck`, no account id in default human table, and
   explicit preferred-next explanation when routing is shown.
6. Define black-box non-blocking proof for server boot/listen, first routed
   request, and quota status render.
7. Define redaction and observability proof across status rows, selection
   explanations, refresh errors, traces/logs, and smoke transcripts.

## Current Phase Update: Revised Spec Ready For Review

The spec-creation pass revised the primary spec to fold in the accepted
spec-review blockers:

- deterministic burn-down score and selector weight contract
- fixed v1 threshold and salvage constants
- explicit crate ownership and dependency rules
- conservative mixed-window collapse
- strict human and machine quota status contracts
- black-box non-blocking proof expectations
- surface-by-surface redaction proof expectations
- end-to-end Codex-through-router WebSocket proof as an implementation gate

Next hard gate:

- rerun `shravan-dev-workflow:spec-review-swarm`
- the dedicated security/trust-boundary lane should be rerun because the prior
  security lane crashed
- do not route to `plan-creation-swarm` unless spec review returns
  `phase_result: complete`

## Current Phase Update: Second Review Findings Folded Into Spec

The second `spec-review-swarm` pass did not accept the first revision. It found
additional blockers around route-band batch assessment ownership, unknown
fallback semantics, WebSocket routing/security order, machine/plain status
surfaces, safe account display, smoke/log redaction, and deterministic scenario
coverage.

The second spec-creation revision now folds those findings into the primary
spec:

- `BurnDownRouteBandAssessment` owns selected pool, weighted candidates, and
  neutral `preferred_next` for CLI status.
- Unknown quota is fallback-only; it never competes with known `usable` or
  `reserve` accounts, but it may preserve conservative partial-headroom ordering
  inside the all-unknown pool.
- `/v1/responses` WebSocket support is explicit, uses the `responses` route
  band, and fails closed for unsupported routes before selection or upstream
  open.
- WebSocket local auth, bounded first-frame validation, selection, credential
  resolution, upstream open, forwarding, and account pinning are ordered
  normatively.
- Default `table` and `plain` output are human-only, while JSON is the explicit
  local machine/debug format.
- Safe account labels or hashes are required by default; raw account id is
  explicit local JSON/debug only.
- Smoke/log/trace transcript evidence is allowlisted and forbids raw bodies,
  full WebSocket first frames, prompts, memory traces, tool args, unsafe labels,
  tokens, auth headers, and secret-store material.
- Scenario D now includes weekly reset facts and numeric expected scoring.
- Scenario E is fallback isolation, not same-pool unknown weighting.

Next hard gate:

- rerun `shravan-dev-workflow:spec-review-swarm`
- do not route to `plan-creation-swarm` unless this second-revision spec review
  returns `phase_result: complete`

## Current Phase Update: Third Review Still Needs Revision

The third `spec-review-swarm` pass reviewed the second-revision spec at commit
`ab89b2bb4e67a2e327a6dfb253cf7de1241ab8f5` with full 837-line coverage. It did
not pass the hard gate.

Accepted blockers:

- `selected_next` overclaims live runtime truth because proxy-owned affinity and
  weighted-deficit state can change the actual next request. The spec must use a
  neutral `preferred_next` projection for shared assessment/status, or define a
  live runtime status surface. The parent reducer chooses `preferred_next` for
  this goal to preserve boundaries and avoid misleading default status.
- Previous-response affinity lacks a first-class fail-closed contract. The spec
  must define durable owner lookup, HTTP/SSE and WebSocket scope, missing or
  unavailable owner behavior, and proof rows before planning.

Accepted important fixes:

- call out the current WebSocket call-order delta as an intentional target
  change
- collapse unsupported WebSocket route taxonomy to an explicit v1
  `unsupported_path` class
- replace ambiguous `account_label` machine output with `safe_account_label`
- resolve unknown fallback ordering when all accounts are unknown
- add live CLI smoke proof for `table`, `plain`, and `json`
- add delayed/failing-refresh proof for first `/v1/responses` WebSocket
- make the default human table invariant explicit

Next hard gate:

- revise the spec through `shravan-dev-workflow:spec-creation-swarm`
- rerun `shravan-dev-workflow:spec-review-swarm`
- do not route to `plan-creation-swarm` until review returns
  `phase_result: complete`

## Current Phase Update: Third Review Findings Folded Into Spec

The spec-creation pass after R3 folded in accepted review findings:

- Pure shared assessment now exposes neutral `preferred_next`, not runtime-exact
  `selected_next`.
- Default status says `preferred next` and explicitly does not claim live next
  request truth after affinity or accumulated weighted-deficit state.
- Previous-response affinity now has a first-class HTTP/SSE and WebSocket
  contract with durable owner lookup and fail-closed owner-resolution failures.
- Weighted fallback is allowed only when no previous-response affinity key is
  present.
- Current WebSocket call-order delta is named as a target change.
- All non-`/v1/responses` WebSocket paths collapse to `unsupported_path`.
- JSON status uses `safe_account_label`, and unsafe configured labels degrade to
  deterministic safe hash/tag.
- Unknown fallback preserves conservative partial-headroom ordering inside the
  all-unknown pool while never competing with known usable/reserve accounts.
- Live-safe CLI status smoke is required for `table`, `plain`, and `json`.
- First valid `/v1/responses` WebSocket routing must prove non-blocking behavior
  under delayed or failing quota refresh.

Next hard gate:

- rerun `shravan-dev-workflow:spec-review-swarm`
- do not route to `plan-creation-swarm` unless the R4 review returns
  `phase_result: complete`

## Current Phase Update: Fourth Review Still Needs Focused Revision

The fourth `spec-review-swarm` pass reviewed commit
`053d3069bad6596d202824c00768e74c1579fe50` with full 961-line coverage. It did
not pass the hard gate, but the remaining issues were focused.

Accepted R4 findings:

- `preferred_next` must be computed from the exact ordered candidate list passed
  to `WeightedDeficitSelector`, not from a prose-only tie rule.
- `next use` needs `available` for same-pool non-preferred accounts.
- Public reason vocabulary must map every assessment outcome to
  `routing_reason`, human phrase, and `next use`.
- V1 public UX is explicitly 5h plus weekly; generic short/long helpers may
  remain internal only.
- Current WebSocket code hardcodes `/v1/responses` selection and lacks
  pre-selection handshake path classification; the spec must name this target
  delta.
- WebSocket first-frame guardrails must freeze 1 MiB, 250 ms,
  `response.create`, local routing/affinity fields only, and upstream-owned full
  schema validation.
- WebSocket redaction proof needs synthetic canary evidence for first-frame and
  request-body non-leakage.

The follow-up spec revision folds these in. Next hard gate remains:

- rerun `shravan-dev-workflow:spec-review-swarm`
- no `plan-creation-swarm` until review returns `phase_result: complete`

## Requirements/proof matrix

Requirement / claim:
Spec captures the actual algorithm and UX contract.
Proof source:
Second-revision spec plus rerun `shravan-dev-workflow:spec-review-swarm` with
`phase_result: complete`.
evidence source:
phase skill result and parent inspection of review artifacts.
freshness guard:
Review must cite the revised spec path and current line coverage.

Requirement / claim:
Implementation plan is true to the goal and accepted spec.
Proof source:
`shravan-dev-workflow:plan-creation-swarm` output with explicit traceability
from every task to goal/spec requirements.
evidence source:
phase skill result and parent inspection of requirements/proof matrix.
freshness guard:
Plan must name the accepted spec review artifact and current git commit.

Requirement / claim:
Plan is not allowed to proceed if it misses full fixes or e2e proof.
Proof source:
`shravan-dev-workflow:plan-review-swarm` with zero accepted blocker findings, or
accepted findings folded back into plan creation.
evidence source:
phase skill result plus parent verification of review findings.
freshness guard:
Review must load both the plan and the accepted spec, not the plan alone.

Requirement / claim:
Runtime routing uses reset-aware burn-down assessment, not minimum-headroom-only
selection.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
unit tests, integration tests, implementation review, and parent command output.
freshness guard:
Tests must run against the implementation branch after final fixes.

Requirement / claim:
Quota status is concise and useful for humans.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
snapshot/golden tests and manual CLI output inspection.
freshness guard:
Golden output must include historical bad cases: noisy per-route rows, `pp`,
`bottleneck`, account ids, and missing preferred-next explanation.

Requirement / claim:
Startup and normal requests do not block on live provider quota refresh.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
smoke test, runtime logs, and parent command output.
freshness guard:
Proof must include boot/listen, first routed request, and quota status render.

Requirement / claim:
Codex can communicate through the router end to end, including WebSocket.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
e2e command transcript using real Codex profile against local router, plus
server logs showing WebSocket path or explicit fallback behavior if fallback is
part of the accepted spec.
freshness guard:
Must run after implementation fixes in the current repo state.

Requirement / claim:
Sensitive account/token material is not leaked in user output, logs, traces, or
test transcripts.
Proof source:
must be defined by plan-creation-swarm.
evidence source:
implementation review, redaction tests, log/trace inspection, and smoke
transcript inspection.
freshness guard:
Must inspect the final emitted output surfaces, not only data structures.

## Hard Gates

- No plan creation until the spec is revised and spec review passes.
- No implementation until plan review passes or accepted plan findings are
  folded back into plan creation.
- No implementation completion claim without unit, integration, smoke, and e2e
  proof gates accounted for.
- No goal completion while implementation review, PR readiness, or WebSocket
  end-to-end proof remains open.
- No checkpoint commit may include unrelated dirty worktree files.

## Blocked Condition

This goal is blocked only if the same blocker repeats under host blocked-state
rules and meaningful progress cannot continue without user input or external
state change. Failed review, failed tests, dirty worktree state, or missing
proof are not completion; they route back to the owning workflow.

## Checkpoint Rhythm

- After revised spec: commit scoped spec artifacts only.
- After accepted spec review: commit scoped review artifacts only.
- After accepted plan: commit scoped plan artifacts only.
- After plan review: commit review artifacts and route to implementation only
  if accepted.
- During implementation: commit only verified slices after proof.
- Before any done claim: run goal closeout audit with matrix rows and current
  evidence.

## R5 Spec Review

Date: 2026-06-23

Reviewed baseline:
`a7dd754cb9ac6016fd767ec2dcb1515af0fed696`

Review worktree:
`/tmp/codex-router-r5-review.zGGh9D`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1014 lines and was read in chunks
1-180, 181-360, 361-540, 541-720, 721-900, and 901-1014.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r5/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- all-unknown fallback is routable but not publicly mapped
- salvage tie ordering is not deterministic enough
- Codex-through-router e2e acceptance is under-specified
- WebSocket preselection failure proof needs a closed matrix
- previous-response affinity extraction is not exact
- unknown/no-window/missing-reset human placeholders can recreate fake `0%`
- JSON status schema is too small to audit the table/routing contract

Revision applied:
The spec now defines fallback next-use semantics, exact salvage tie key,
unknown/no-data placeholders, expanded JSON audit shape, exact
`previous_response_id` affinity extraction, WebSocket preselection failure
matrix, and installed-Codex-through-router e2e acceptance.

## R6 Spec Review

Date: 2026-06-23

Reviewed baseline:
`8dab4631a8f2cdabfaaedb0be233f633f15fa04d`

Review worktree:
`/tmp/codex-router-r6-review.8EyxlP`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1106 lines and was read in chunks
1-200, 201-400, 401-600, 601-800, 801-1000, and 1001-1106.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r6/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- WebSocket first-frame local field allowlist was not exact
- account eligibility ownership was overloaded
- unknown fallback final `routing_reason` conflicted with raw evidence reason
- partial v1 5h/weekly window sets were not normatively collapsed

Revision applied:
The spec now makes `/v1/responses` route band path-derived before selection,
allows only top-level `type` and top-level `previous_response_id` first-frame
reads before selection, splits `quota_evidence_reason` from final
`routing_reason`, defines `missing_expected_window`, and separates proxy fact
adaptation/runtime enforcement from pure burn-down exclusion/classification.

## R7 Spec Review

Date: 2026-06-23

Reviewed baseline:
`5dd58c8259c30bdce0da84a28aa9704492379584`

Review worktree:
`/tmp/codex-router-r7-review.8GCXy0`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1151 lines before R7 fixes and was
read in chunks 1-230, 231-460, 461-690, 691-920, and 921-1151.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r7/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- assessment inclusion contradicted excluded-account status rows
- previous-response affinity lacked an owner-record creation/version contract
- JSON status contract was a field inventory, not a normative envelope
- raw local JSON `account_id` conflicted with shared-artifact redaction proof
- status proof lacked structural guardrails for one logical row per account and
  no unrelated route-band noise

Revision applied:
The spec now builds assessments for every supplied account fact row, keeps
excluded/blocked rows out of `weighted_candidates`, defines previous-response
owner-record shape and pin-write rules, distinguishes raw local JSON from
redacted shared artifacts, defines a normative JSON envelope, and adds structural
status proof guardrails.

## R8 Spec Review

Date: 2026-06-23

Reviewed baseline:
`c8c02e1886d06c344aa55d35288cf844daacb23b`

Review worktree:
`/tmp/codex-router-r8-review.RBmbZ8`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1245 lines before R8 fixes and was
read in chunks 1-250, 251-500, 501-750, 751-1000, and 1001-1245.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r8/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- `affinity_key_hash` algorithm, encoding, keyedness, truncation, and collision
  behavior were underspecified
- previous-response owner route eligibility was not mapped to burn-down
  availability classes

Revision applied:
The spec now defines full-length lowercase-hex HMAC-SHA-256 affinity hashes
using router-owned secret material, one shared helper before storage/logging/
tracing/audit, hard schema cutover with no raw-key fallback, duplicate ambiguity
fail-closed behavior, and continuation owner validity limited to `usable` or
`reserve` owners.

## R9 Spec Review

Date: 2026-06-23

Reviewed baseline:
`5e39282dea9defdfabff60af07593e0605f5592e`

Review worktree:
`/tmp/codex-router-r9-review.68OnKV`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1277 lines before R9 fixes and was
read in chunks 1-260, 261-520, 521-780, 781-1040, and 1041-1277.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r9/review-ledger.md`

Phase result:
needs_revision

Accepted finding:

- `router_affinity_hash_secret` lifecycle and rotation behavior were
  underspecified for durable owner lookup

Revision applied:
The spec now generates the affinity hash secret once per router root, persists it
independently from bearer/account credential rotation, forbids v1 rotation,
keeps it stable across restarts and refreshes, and requires owner rows to be
ignored or purged with continuations failing closed when the secret is missing,
unreadable, or replaced.

## R10 Spec Review

Date: 2026-06-23

Reviewed baseline:
`71487af`

Review worktree:
`/tmp/codex-router-r10-review.lHVDkc`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1294 lines before R10 fixes and was
read in chunks 1-260, 261-520, 521-780, 781-1040, and 1041-1294.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r10/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- previous-response affinity needed concrete core/secret-store/state/proxy API
  ownership for hash-secret storage, HMAC construction, repository methods, and
  schema cutover
- route-band policy needed a selection-owned registry covering every currently
  classified route band
- `accounts[]` ordering and `weighted_candidates[]` ordering contradicted each
  other and needed separate contracts
- safe account label/hash sanitization needed one shared owner
- `router_affinity_hash_secret` needed to be in global security assets,
  forbidden emission surfaces, and proof expectations
- public `routing_reason` needed deterministic precedence when preferred
  explanation predicates overlap
- secret-unavailable behavior for response-creating routes needed to be explicit
- installed-Codex e2e needed to pin generated profile local auth to
  `env_http_headers`, not `env_key` or Authorization fallback

Revision applied:
The spec now defines core-owned safe-label and affinity helpers,
secret-store-owned hash-secret loading, state-owned hashed owner records,
proxy-owned affinity orchestration, route-band policy registry, split output
ordering, routing-reason precedence, affinity-secret-unavailable failure,
hash-secret redaction proof, and generated-profile local auth proof.

## R11 Spec Review

Date: 2026-06-23

Reviewed baseline:
`66dfe14`

Review worktree:
`/tmp/codex-router-r11-review.8XkrYg`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1447 lines before R11 fixes and was
read in chunks 1-300, 301-600, 601-900, 901-1200, and 1201-1447.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r11/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- public `routing_reason` lacked a weekly-reset-imminent reason for
  long-window salvage, so Scenario B could hide why the preferred account won
- smoke transcript redaction still allowed individual non-allowlisted
  WebSocket first-frame/body fields to leak
- previous-response affinity needed an explicit cutover away from
  `codex-router-selection::affinity` and raw `AffinityKey`
- route-band identity needed a shared source of truth between proxy route
  classification and selection policy lookup
- affinity hash-secret storage needed concrete secret-store API, stable key,
  entropy/encoding, typed return, and redacted error contract
- safe-label helper needed concrete unsafe predicates and redacted tag format

Revision applied:
The spec now defines `preferred_weekly_reset_soon`, smoke transcript
first-frame/body field allowlisting, previous-response raw-key cutover,
core-owned `RouteBand`, affinity secret-store API/key/encoding/error contract,
and `SafeAccountLabel` semantics with deterministic redacted tag format.
