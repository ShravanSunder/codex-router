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

Resolve these paths in the current checkout or review worktree for the git
commit being reviewed:

- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r16/review-ledger.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-revision-2026-06-23-r17/swarm-ledger.md`
- `tmp/workflow-state/2026-06-23-quota-burndown-routing/details.md`
- `tmp/workflow-state/2026-06-23-quota-burndown-routing/events.jsonl`

Earlier review ledgers remain historical context only. The latest review ledger
is authoritative for the next workflow transition.

Historical phase updates below are retained as a log, not active instructions.
When older sections mention `phase_result` or rejected generated-profile auth
shapes, the active source of truth is the required-reading list above, the
latest orchestrator event, and the current primary spec.

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
- do not route to `plan-creation-swarm` unless spec review returns a
  parent-verified verdict of `ready`

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
- do not route to `plan-creation-swarm` unless spec review returns a
  parent-verified verdict of `ready`

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
- do not route to `plan-creation-swarm` until review returns a parent-verified
  verdict of `ready`

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
- do not route to `plan-creation-swarm` unless spec review returns a
  parent-verified verdict of `ready`

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
- no `plan-creation-swarm` until review returns a parent-verified verdict of
  `ready`

## Requirements/proof matrix

Requirement / claim:
Spec captures the actual algorithm and UX contract.
Proof source:
Second-revision spec plus rerun `shravan-dev-workflow:spec-review-swarm` with a
parent-verified verdict of `ready`.
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
unit tests for per-window math, account collapse, selected pool choice,
neutral selector ordering, `preferred_next`, route-level
`unsupported_route_band`, previous-response affinity fail-closed paths, and
proxy integration tests proving runtime selection consumes
`BurnDownRouteBandAssessmentResult.weighted_candidates`.
evidence source:
unit/integration command output, implementation review, and parent inspection of
changed proxy/selection boundaries.
freshness guard:
Tests must run against the implementation branch after final fixes.

Requirement / claim:
Quota status is concise and useful for humans.
Proof source:
CLI renderer golden/snapshot tests for table and plain output, JSON schema tests
for machine output, and live-safe CLI smoke over persisted state. Required bad
case negatives: noisy per-route rows, `pp`, `bottleneck`, default `account_id`,
raw scores, token-like strings, and missing preferred-next explanation.
evidence source:
snapshot/golden output, JSON assertion output, smoke transcript, and parent
manual CLI inspection.
freshness guard:
Golden output must include historical bad cases: noisy per-route rows, `pp`,
`bottleneck`, account ids, and missing preferred-next explanation.

Requirement / claim:
Startup and normal requests do not block on live provider quota refresh.
Proof source:
black-box smoke proving server boot/listen readiness while refresh is delayed or
failing, first routed HTTP/SSE and WebSocket requests using persisted selector
rows while refresh is delayed or failing, and `quota status` rendering
last-known state immediately with needs-refresh/stale indications.
evidence source:
smoke test, runtime logs, and parent command output.
freshness guard:
Proof must include boot/listen, first routed request, and quota status render.

Requirement / claim:
Codex can communicate through the router end to end, including WebSocket.
Proof source:
installed Codex CLI e2e against a served local router and mock upstream with a
generated codex-router profile using
`env_key = "CODEX_ROUTER_TOKEN"`, which installed Codex sends as local
`Authorization: Bearer`. The fixture must exercise HTTP/SSE and WebSocket
`/v1/responses`, reset-aware account choice, WebSocket selected-account pinning,
local auth carrier validation via safe enum/boolean, local auth stripping before
upstream open, and redacted transcript artifacts.
evidence source:
e2e command transcript, mock upstream assertions, router logs, and redacted
smoke artifact inspection.
freshness guard:
Must run after implementation fixes in the current repo state. WebSocket is not
optional for this proof gate.

Requirement / claim:
Sensitive account/token material is not leaked in user output, logs, traces, or
test transcripts.
Proof source:
safe-label helper tests, JSON redaction tests, log/trace canary tests, WebSocket
first-frame/body canary tests, local-auth negative tests, affinity-secret
redaction tests, and smoke transcript negative assertions for tokens, auth
headers, account ids where forbidden, unsafe labels, prompts, tool args, raw
bodies, and secret-store material.
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
- Historical rejected finding: installed-Codex e2e was once pointed at an old
  explicit-header generated-profile shape; later review rejected that. Active
  contract is
  `env_key = "CODEX_ROUTER_TOKEN"` and local `Authorization: Bearer`.

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

## R12 Spec Review

Date: 2026-06-23

Reviewed baseline:
`195cb74`

Review worktree:
`/tmp/codex-router-r12-review.8otShY`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1545 lines before R12 fixes and was
read by the parent in chunks 1-320, 321-640, 641-960, 961-1120, 1121-1280,
1281-1440, and 1441-1545. Three review lanes also reported full-read coverage.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r12/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- local router auth needed an explicit accepted `X-Codex-Router-Token` surface
  and forbidden fallback surfaces for HTTP/SSE, WebSocket, and generated Codex
  profiles
- WebSocket routing needed to load/create `router_affinity_hash_secret.v1`
  before selection because `/v1/responses` can create previous-response owner
  records even when the first request has no incoming affinity
- unknown fallback reason precedence made fallback reasons unreachable
- shared assessment output needed to carry status presentation fields instead
  of leaving CLI/JSON to rederive routing semantics
- route-band policy lookup needed a route-level unsupported-band result surface
- quota refresh lifecycle needed normative startup, scheduling, failure, and
  persistence ownership rules
- goal details needed current required-reading paths and concrete proof rows

Revision applied:
The spec now defines `BurnDownRouteBandAssessmentResult`, selection-owned
presentation fields, selected-pool-aware fallback reason precedence, a quota
refresh lifecycle, local-auth ingress contract, WebSocket affinity-secret
preselection, and the required proof rows. Goal details now point at current
checkout-relative sources and concrete proof families.

## R13 Spec Review

Date: 2026-06-23

Reviewed baseline:
`5e5a1c4`

Review worktree:
`/tmp/codex-router-r13-review.jSdi0u`

Coverage:
`reset-aware-burndown-routing-spec.md` was 1665 lines before R13 fixes and was
read by the parent in chunks 1-280, 281-560, 561-840, 841-1120, 1121-1400, and
1401-1665. Three review lanes also reported full-read coverage.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r13/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- local-auth rejection needed to reject mixed-carrier requests even when the
  accepted `X-Codex-Router-Token` header is present
- HTTP/SSE response-creating and previous-response-capable routes needed an
  explicit affinity-secret preselection order
- WebSocket preselection wording needed to forbid parsing any additional
  first-frame/body fields before selection
- route-band policy lookup needed one owner: selection, not caller-supplied
  `route_band_policy`
- unsupported route-band result needed a stable payload and machine reason
- refresh persistence needed a durable `quota_refresh_status` shape and
  success/failure transition rules
- `window_slots.source_window_ids` had no input contract and needed removal from
  v1

Revision applied:
The spec now defines selection-owned `assess_route_band(...)`,
`UnsupportedRouteBandAssessment`, `quota_refresh_status`, v1 `window_slots`
without source ids, mixed-carrier auth failure, HTTP/SSE routing order, tighter
WebSocket preselection wording, and proof rows for each contract.

## R14 Spec Review

Date: 2026-06-23

Reviewed baseline:
`f104ff9`

Coverage:
`reset-aware-burndown-routing-spec.md` is 1788 lines and was read by the parent
in chunks 1-320, 321-640, 641-960, 961-1280, 1281-1600, and 1601-1788. Three
review lanes also reported coverage across their assigned surfaces.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r14/review-ledger.md`

Phase result:
needs_revision

Accepted findings:

- top-level requirement trace is incomplete for local auth, generated profile,
  affinity-secret, WebSocket preselection, and smoke-redaction cutovers
- Historical rejected finding: an older spec required an explicit-header
  generated-profile shape with `X-Codex-Router-Token`. Active contract is
  generated-profile
  `env_key = "CODEX_ROUTER_TOKEN"` with local `Authorization: Bearer`, while
  `X-Codex-Router-Token` remains manual/compat ingress.
- WebSocket preselection wording conflicts with the current direct-payload
  branch that reads fields such as `model`, `input`, and `stream`
- unknown-quota fallback/probe semantics are not first-class in the route
  result contract or selected-pool model
- route-level result fields are inconsistent across `preferred_next`,
  `preferred_next_account_id`, `route_result`, and unsupported-band payloads
- refresh staleness, status/JSON DTO ownership, smoke transcript redaction, and
  affinity-secret cutover order need sharper proofable contracts

Next hard gate:
Return to `shravan-dev-workflow:spec-creation-swarm`; do not proceed to
`plan-creation-swarm` until the spec is revised and another spec review passes.

## R15 Spec Revision

Date: 2026-06-23

Phase:
spec-creation-swarm revision after R14.

Revision artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-revision-2026-06-23-r15/swarm-ledger.md`

Lanes:

- auth/profile compatibility: generated Codex profile remains
  `env_key = "CODEX_ROUTER_TOKEN"`; installed Codex authenticates to the local
  router with `Authorization: Bearer` for HTTP/SSE and WebSocket
- selection envelope and cooldown: route-level output now uses one canonical
  `route_result`, `selected_pool`, `selected_pool_reason`,
  `preferred_next_account_id`, `weighted_candidates`, and `accounts` shape;
  runtime holds/affinity survive only when the account remains in current
  `weighted_candidates`
- status, refresh, and transcript safety: refresh success/failure repository
  operations are explicit, failed refresh preserves last-known selector rows,
  and smoke transcripts forbid raw or derived non-allowlisted first-frame fields

Revision applied:
The spec now corrects the installed-Codex local-auth/profile contract, allows
bounded direct WebSocket response-create first-frame structural checks without
logging raw values, makes unknown fallback and unsupported route-band payloads
first-class in the route result contract, adds cooldown/pinning proof rows, and
adds refresh repository operation/state-transition proof rows.

Next hard gate:
Run `shravan-dev-workflow:spec-review-swarm` against R15. Do not proceed to
`plan-creation-swarm` until that review passes.

## R15 Spec Review

Date: 2026-06-23

Reviewed baseline:
`aa1ed57`

Coverage:
`reset-aware-burndown-routing-spec.md` is 1936 lines and was read by the parent
in chunks 1-400, 401-800, 801-1200, 1201-1600, and 1601-1936. Four review
lanes also reported full assigned coverage.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r15/review-ledger.md`

Verdict:
needs revision

Accepted blockers:

- refresh staleness has no canonical read path, formula, API owner, or
  consumer for `last_error_class`
- previous-response affinity contradicts cooldown/pinning for reserve owners
- WebSocket request-body token rejection contradicts the non-allowlisted
  first-frame parsing rule
- workflow/source-of-truth details still contain stale required-reading and
  generated-profile auth guidance

Accepted important findings:

- route-result envelope still differs across supported, unsupported, JSON,
  status, and runtime surfaces
- current-state WebSocket evidence overstates the implementation delta
- mixed-carrier local auth needs a carrier-preserving input boundary
- installed-Codex transcript redaction needs explicit cutover language
- generated-profile bearer-auth e2e proof needs a named safe observable or an
  explicit split between ingress tests and e2e proof
- tail workflow wording should use parent-verified spec-review verdict
  `ready`, not an old `phase_result` completion signal

Next hard gate:
Return to `shravan-dev-workflow:spec-creation-swarm`; do not proceed to
`plan-creation-swarm` until the spec is revised and another spec review returns
a parent-verified `ready` verdict.

## R16 Spec Revision

Date: 2026-06-23

Phase:
spec-creation-swarm revision after R15.

Revision artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-revision-2026-06-23-r16/swarm-ledger.md`

Revision applied:

- refresh staleness is now a `codex-router-state` repository read-model overlay
  with formula `last_success + max(refresh_interval * 2, 600)`
- previous-response affinity can reuse `usable` or `reserve` owners, even
  outside the current selected pool, and fails closed for unknown/blocked/
  excluded/exhausted/ineligible/missing-credential/stale-generation owners
- WebSocket first-frame auth-smuggling hard-fails only on forbidden top-level
  auth-carrier field names and does not scan nested prompt/body values
- route-result inventory now includes `route_band` and full
  `selected_pool_reason` domain across ok and unsupported branches
- local auth validation now has an input contract that preserves both accepted
  carriers plus forbidden-carrier presence until mismatch checks run
- generated-profile e2e bearer proof now uses safe observables:
  `local_auth_carrier=authorization_bearer` and `local_auth_validated=true`
- current-state WebSocket evidence and tail workflow verdict wording were
  corrected
- active required-reading and e2e proof rows in this details file now point at
  R15/R16 and the `env_key`/Authorization bearer contract

Next hard gate:
Run `shravan-dev-workflow:spec-review-swarm` against R16. Do not proceed to
`plan-creation-swarm` until that review returns a parent-verified `ready`
verdict.

## R16 Spec Review

Date: 2026-06-23

Phase:
spec-review-swarm review of R16.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r16/review-ledger.md`

Phase result:
needs_revision

Accepted blockers:

- refresh read overlay still exposed contradictory selector repository APIs
- WebSocket/HTTP local-auth and auth-smuggling ownership was still fuzzy
- installed-Codex generated-profile bearer proof was optional
- HTTP/SSE body-token rejection had no exact inspection boundary
- goal details still contained stale active source-of-truth guidance

Accepted important fixes:

- raw unknown quota evidence reasons needed to be evidence-only, not public
  routing reasons
- legacy selector rows with no refresh metadata needed first-read semantics
- proxy runtime selection needed a DTO carrying the shared route-result envelope
- affinity hit side effects on weighted-deficit state and route-band holds
  needed to be normative

## R17 Spec Revision

Date: 2026-06-23

Phase:
spec-creation-swarm revision after R16.

Revision artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-revision-2026-06-23-r17/swarm-ledger.md`

Revision applied:

- canonical state read API is
  `selector_inputs_for_route_band(route_band, now_unix_seconds)`
- legacy selector rows without refresh metadata are stale on first post-upgrade
  read before successful refresh
- unknown public routing reasons are pool-based, with raw causes preserved in
  `quota_evidence_reason`
- `RuntimeSelectedAccountDecision` carries the shared assessment envelope
- affinity hit side effects are specified
- HTTP/SSE body and WebSocket first-frame auth-smuggling checks are narrow
  top-level JSON field-name validators
- installed-Codex bearer receipt proof is mandatory and audit-safe
- primary spec is 1990 lines, under the artifact cap

Next hard gate:
Run `shravan-dev-workflow:spec-review-swarm` against R17. Do not proceed to
`plan-creation-swarm` until that review returns a parent-verified `ready`
verdict.

## R17 Spec Review

Date: 2026-06-23

Phase:
spec-review-swarm review of R17.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r17/review-ledger.md`

Phase result:
needs_revision

Accepted blockers:

- proof expectations were too `/v1/responses`-centric and did not hard-gate
  every routed API: HTTP/SSE `/v1/responses`, WebSocket `/v1/responses`,
  `/v1/models`, `/v1/memories/trace_summarize`, `/v1/responses/compact`, and
  unsupported path rejection
- `unsupported_path` proxy-edge rejection and `unsupported_route_band`
  assessment/status misses were collapsed
- runtime account-selection side effects were not exact enough for weighted
  fallback, cooldown reuse, previous-response affinity, WebSocket connection
  pins, route-band holds, and durable owner writes
- WebSocket direct-payload validation did not match the current fail-closed
  behavior for non-empty string `model`, top-level array `input`, and literal
  `stream=true`

Accepted important fixes:

- refresh-status reads needed an exact sorted return contract and legacy missing
  metadata semantics
- `RuntimeSelectedAccountDecision` duplicated selected-pool state already owned
  by the shared assessment envelope
- weighted-deficit state and cooldown holds needed explicit route-band
  partitioning
- cooldown reuse must debit weighted-deficit state, but previous-response
  affinity must not
- installed-Codex bearer proof needed transport-specific local-auth receipt
  fields
- HTTP/SSE body auth-smuggling proof needed to be scoped to every supported JSON
  POST route

## R18 Spec Revision

Date: 2026-06-23

Phase:
spec-creation-swarm revision after R17.

Revision artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-revision-2026-06-23-r18/swarm-ledger.md`

Revision applied:

- added a normative route/API inventory and route-native e2e proof requirement
  for every currently routed surface, not just `/v1/responses`
- split raw classifier rejection `unsupported_path` from classified route-band
  policy miss `unsupported_route_band`
- closed the refresh-status read API as
  `quota_refresh_statuses_for_route_band(route_band) ->
  BTreeMap<AccountId, QuotaRefreshStatusView>`
- removed duplicate `assessment_selected_pool` from the runtime selected account
  DTO
- made route-band partitioning of weighted-deficit state and cooldown holds
  normative
- added a runtime side-effects table: weighted fallback advances fairness,
  cooldown reuse records a fairness debit, previous-response affinity does not
  advance fairness, and upstream response ids alone write durable owner records
- required affinity continuation to remain a runtime continuity override, not a
  way to move reserve owners into weighted candidates
- made installed-Codex local-auth e2e receipts transport-specific for HTTP/SSE
  and WebSocket
- tightened WebSocket direct-payload compatibility to non-empty string `model`,
  top-level array `input`, and literal `stream=true`

Current implementation deltas that the plan must force with tests:

- `RepositoryBackedAccountSelector::select_affinity_owner` currently requires
  the owner to be in `weighted_candidates` and calls
  `WeightedDeficitSelector::record_selection`; the spec requires successful
  previous-response affinity to avoid fairness mutation and to allow valid
  reserve-owner continuation outside the currently weighted pool
- `BurnDownRouteBandAssessmentInput` currently still exposes caller-supplied
  policy; the spec requires policy lookup/registration to be owned by the
  selection crate so proxy/status callers cannot drift by passing alternate
  route-band policy
- proxy tests already cover route-band partitioning and cooldown reuse, but the
  plan still needs explicit red/green tests for affinity-no-fairness-mutation,
  reserve-affinity continuity, per-route API e2e, and unsupported path versus
  unsupported route-band separation

Next hard gate:
Run `shravan-dev-workflow:spec-review-swarm` against R18. Do not proceed to
`plan-creation-swarm` until that review returns a parent-verified `ready`
verdict.

## R18 Spec Review

Date: 2026-06-23

Phase:
spec-review-swarm review of R18.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r18/review-ledger.md`

Phase result:
needs_revision

Accepted blockers:

- HTTP/SSE routing order still used `unsupported_route_band` for raw classifier
  misses and did not cover every routed HTTP API
- wrong HTTP methods on otherwise supported paths were not included in the
  black-box fail-closed proof gate
- shared assessment/result contract was still ambiguous between enum payloads
  and one flat envelope
- WebSocket invalid local auth and unsupported path proof did not require
  handshake/connect failure or another non-101 local rejection

Accepted important fixes:

- routes marked not previous-response capable needed explicit
  `previous_response_id` behavior
- `BurnDownAccountInput` should not carry per-account `route_band`
- unsupported-route-band JSON should be internal/test-only in v1

## R19 Spec Revision

Date: 2026-06-23

Phase:
spec-creation-swarm revision after R18.

Revision artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-revision-2026-06-23-r19/swarm-ledger.md`

Revision applied:

- chose one flat `BurnDownRouteBandAssessmentResult` envelope with
  `route_result` as discriminator
- removed per-account `route_band` from pure assessment input
- made unsupported-route-band JSON internal/test-only, not a new v1 user
  command
- made HTTP/SSE routing order apply to every supported HTTP route
- split raw `unsupported_path` classifier misses from classified
  `unsupported_route_band` policy misses
- scoped affinity to previous-response-capable routes only; non-capable routes
  pass top-level `previous_response_id` through as normal upstream payload after
  local auth and auth-smuggling checks
- required wrong-method black-box proof on supported paths
- required WebSocket invalid-auth/unsupported-path proof to observe
  handshake/connect failure or another non-101 local rejection

Next hard gate:
Run `shravan-dev-workflow:spec-review-swarm` against R19. Do not proceed to
`plan-creation-swarm` until that review returns a parent-verified `ready`
verdict.

## R19 Spec Review

Date: 2026-06-23

Phase:
spec-review-swarm review of R19.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r19/review-ledger.md`

Phase result:
needs_revision

Accepted blockers:

- stale public references to `BurnDownRouteBandAssessment.*` undermined the
  flat `BurnDownRouteBandAssessmentResult` envelope cutover
- HTTP/SSE and WebSocket routing order made shared assessment look conditional
  on no affinity, contradicting selected-pool-before-affinity and current
  selector flow

What held:

- WebSocket/harness proof lane returned ready
- `unsupported_path` versus `unsupported_route_band`, wrong-method proof,
  unsupported-route-band JSON scope, route-scoped affinity, and non-capable
  `previous_response_id` pass-through held

## R20 Spec Revision

Date: 2026-06-23

Phase:
spec-creation-swarm revision after R19.

Revision artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-revision-2026-06-23-r20/swarm-ledger.md`

Revision applied:

- removed remaining `BurnDownRouteBandAssessment.*` public-surface references
- made HTTP/SSE build the shared route-band assessment before route-scoped
  affinity enforcement
- made WebSocket build the shared `responses` assessment before route-scoped
  affinity enforcement
- aligned HTTP/SSE call-order proof wording so assessment precedes optional
  affinity

Next hard gate:
Run focused `shravan-dev-workflow:spec-review-swarm` against R20. Do not
proceed to `plan-creation-swarm` until that review returns a parent-verified
`ready` verdict.

## R20 Spec Review

Date: 2026-06-23

Phase:
spec-review-swarm focused review of R20.

Review artifacts:
`tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r20/review-ledger.md`

Phase result:
ready

Review result:

- the focused review accepted no blockers
- R20 closed the stale `BurnDownRouteBandAssessment.*` public-reference issue
- R20 closed the HTTP/SSE and WebSocket selector-order issue by requiring
  shared assessment before optional route-scoped affinity
- remaining gates are implementation proof gates, not spec-review blockers

Next hard gate:
Run `shravan-dev-workflow:plan-creation-swarm` and produce a plan that maps the
accepted spec to explicit unit, integration, smoke, installed-Codex HTTP, and
installed-Codex WebSocket proof gates before implementation begins.
