# Plan Revision Ledger

Date: 2026-06-22
Workflow: `shravan-dev-workflow:plan-creation-swarm`
Goal id: `2026-06-22-codex-router-quota-oauth-runtime`

## Source Coverage

- Spec: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`, 497 lines, read in chunks `1-170`, `171-340`, `341-497`.
- Umbrella plan after current revision:
  `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`, 360 lines.
- Plan 1A after current revision:
  `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`, 558 lines.
- Plan 1B after current revision:
  `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`, 703 lines.
- Review receipt:
  `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-e7b55fc.md`, 309 lines.
- Current repo evidence: branch `feature/initial-codex-router`, reviewed input
  plan commit `e7b55fc31c7b7a53507eacd8c87ef0201729ba17`, dirty worktree with
  pre-existing product/doc changes, remote push blocker `repository not found`.

## Plan-Creation Lanes

### Post-Review Revision Lanes

- `post-918db95-validation-proof`: accepted. Plan 1A now has row `1A-00h`
  proving the exact-one helper with a real test and missing sentinel before
  handoff; activation profile rows assert the exact
  `"X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN"` header map; Plan 1B rows
  `1B-23c`, `1B-26`, and `1B-27a` now use executable commands rather than
  prose; final freshness row `1B-27b` reruns the reopened Plan 1A structural
  guards from the current checkout.
- `post-918db95-codebase-boundary-execution`: accepted. Plan 1B now uses a
  single acyclic dependency direction with `codex-router-quota` depending on
  `codex-router-auth` and never the reverse, chooses `codex-router-state` as
  the durable owner for quota leases, turn-state replay, and previous-response
  ownership, adds `routes.rs` and `local_auth.rs` to T9, and replaces vague task
  buckets with exact file paths.
- `post-918db95-scope-and-proof-fit`: accepted. Only current receipt artifacts
  were updated: the three durable plan docs, this revision ledger, and
  workflow-state details/events. Historical restructure and review artifacts
  remain untouched as audit history.

- `post-e7b55fc-proof-exactness`: accepted. Plan 1A row `1A-00h` and Plan
  1B row `1B-23c` now use count-based exact-one commands that fail if the
  real test is missing or duplicated. Plan 1B row `1B-26` now rejects any
  `approval:` or `result:` key while the live gate is `not-run`.
- `post-e7b55fc-structural-freshness`: accepted. Row `1B-07b` now forbids the
  process-local secret-store refresh lease only in quota/CLI refresh-cycle
  entrypoints while requiring state-owned refresh/quota lease APIs. Rows
  `1B-27a` and `1B-27b` now use explicit file existence checks and avoid
  inverted missing-path success for planned factory/adapter files.
- `post-e7b55fc-boundary-scope`: accepted. Plan 1B now includes
  `crates/codex-router-auth/src/lib.rs` in the quota provider-client migration
  write scope, and Plan 1A now requires an A2 structural proof that the live
  selector adapter used by `server.rs` has moved behind `account_selection.rs`
  before Plan 1B T8 starts.
- `post-e7b55fc-security-reliability`: accepted. Plan 1B now adds WebSocket
  previous-response restart/unknown-owner proof row `1B-14c`, and structural
  row `1B-13c` clarifies that selection turn-state/affinity code is codec-only
  and durable replay/previous-response persistence is state-owned.

- `post-734554d-preflight-and-smoke`: accepted. Plan 1A/1B now include an
  exact-one preflight policy that treats raw `cargo test -- --list` output as a
  list source only, plus final closeout rows for generated-profile/token/WebSocket
  installed smoke and explicit preflight-helper proof.
- `post-734554d-activation-proof`: accepted. The umbrella no longer defers
  activation/profile proof, and Plan 1A now has exact rows for profile print,
  token export/doctor redaction, dry-run preview, and approved temp-home profile
  write.
- `post-734554d-boundary-ownership`: accepted. Plan 1A/1B now make selection
  state-free behind a state-to-selector adapter, add proxy module declaration
  write ownership, require deletion/redefinition of the legacy proxy selector
  token carrier, and add selector dependency-direction proof.
- `post-734554d-quota-runtime-ownership`: accepted. Plan 1B now names quota
  provider-client ownership, forbids process-local `RefreshLeaseManager` for
  quota one-writer behavior, and adds structural proof rows for both.
- `post-734554d-affinity-and-protocol-proof`: accepted. Plan 1B now requires
  previous-response restart/unknown-owner behavior, unsupported WebSocket route
  fail-closed proof, and first-frame WebSocket proof for affinity and
  non-affinity routing before upstream open.
- `post-734554d-live-and-structural-proof`: accepted. Plan 1B now scopes the
  live OAuth gate to the current gate block and replaces line-filtered backend
  construction proof with grouped production/factory/test-only structural
  checks.

- `post-8fb965f-base-story`: accepted. The umbrella now keeps `8fb965f` as the
  reviewed base and makes `account` / non-live `quota` command surfaces planned
  implementation work unless dirty product code is explicitly promoted or
  carried forward with receipts.
- `post-8fb965f-rotation`: accepted. Plan 1B now owns explicit precommit
  auth/quota rotation plus a no-router-retry row for transport, timeout,
  overload, DNS, reset, cancellation, and post-commit stream failures.
- `post-8fb965f-proof-tightening`: accepted. Plan 1A/1B now separate exact-test
  stale guards from structural/search rows, replace unsafe resolver-bypass
  filters, scope selector token proof to account-decision modules, and split
  backend construction proof from provider-token egress proof.
- `post-8fb965f-security-proof`: accepted. Plan 1B now adds WebSocket
  empty/wrong/old local-token proof, restart replay proof, stricter live-gate
  proof, smoke transcript redaction/schema proof, and `codex-router-core` gate.
- `post-8fb965f-ownership`: accepted. Plan 1B now has a task-to-file ownership
  table and Plan 1A names backend-neutral secret-store trait/factory ownership
  plus the production `AuditFailureReporter` diagnostic channel.

- `post-460b51e-source-freeze`: accepted. Umbrella, Plan 1A, and Plan 1B now
  require dirty source inputs to be committed/promoted before execution or
  carried forward with path, source commit/head, checksum or byte count,
  working-tree line count, execution-base line count, and normative flag.
- `post-460b51e-credential-contract`: accepted. Plan 1A now chooses one
  versioned bundled credential secret plus SQLite active-generation pointer
  contract, names secret-store/state/auth ownership, and adds exact active-bundle
  resolver proof.
- `post-460b51e-selector-resolver-separation`: accepted. Plan 1A now requires
  selector decisions to carry account/route decisions only; provider token
  material belongs to resolver output immediately before egress.
- `post-460b51e-proof-exactness`: accepted. Structural rows now use explicit
  copy-pasteable commands; T1 has explicit account/quota/serve rows; installed-smoke
  rows 1B-23, 1B-23a, 1B-24, and 1B-25 are mandatory; only live row 1B-26 may record
  `not-run: approval required`.
- `post-460b51e-runtime-reliability`: accepted. Plan 1B now names a
  state-owned response-family publication API/generation-fence requirement,
  two-independent-connection stale-owner proof, replay-state owner packet, and
  final runtime egress resolver-bypass row across quota, CLI, and proxy paths.

- `revision-codebase-boundary`: accepted. The revised plans now make
  `codex-router-auth` the resolver owner, `codex-router-state` the selector
  projection owner, and `codex-router-quota` the quota runtime owner, with
  manifest changes in scope rather than hidden task-local amendments.
- `revision-validation-proof`: accepted. Proof rows now use full libtest paths,
  exact `nextest` command forms, per-behavior route/local-auth/smoke rows, and
  explicit existing-vs-new proof ownership.
- `revision-execution-order`: accepted. Execution is now a hard serial chain
  with Gate 0a/0b, repeated dirty-tree checkpoint proof at A1/A2/B0/B1/final,
  and fresh-worktree `tmp/` lifecycle carry-forward.
- `revision-security-reliability`: accepted. Plans now own replay-scope
  affinity, local bearer split proof, response-alias family atomicity,
  cross-process quota refresh stale-owner recovery, and surfaced audit append
  failure diagnostics.

- `oauth-account-ux-scope`: accepted. Plan 1A/1B can proceed only as quota/runtime prerequisite work; onboarding-complete multi-account auth requires reviewed Plan 2 with OS keyring/Keychain, device-code login, logout/remove, mocked UX proof, and approval-gated live proof.
- `validation-proof-exactness`: accepted. Matrices now use proof owner, exact preflight list command, exact execution command, expected observation, stale-proof guard, and red/green column. Missing tests are named explicitly as implementation deliverables.
- `execution-order-write-scope`: accepted. Default execution is serial, dirty-tree isolation is a gate, Plan 1B early-start exception is removed, merge gates A1/A2/B0/B1 are executable receipts, and WebSocket write scope is explicit.
- `security-reliability-affinity`: accepted. Plans now own resolver bypass guard, audit JSONL allowlist proof, route-band invalidation, cross-process quota-refresh one-writer behavior, same-turn/previous-response affinity, and local bearer-token lifecycle proof.

Lane reasoning-effort policy:

- OAuth/account UX, validation/proof, execution/write-scope, and security/reliability were all high-risk cross-module planning lanes and were assigned high reasoning.

## Parent Decisions

- Plan 1 remains split into Plan 1A and Plan 1B. The split is still correct, but execution is serial by default.
- Plan 2 remains non-executable here, but it is no longer optional prose. It is a required blocker before onboarding-complete or release-ready multi-account auth claims.
- `account select` is not a Plan 1 command. Runtime account choice stays selector-owned.
- Account inventory is `account list`; quota state is `quota status`. No separate account `status` command is planned in Plan 1.
- `code_review` remains status/quota state only unless a later spec promotes it to a routed selector band. Credential mutation should invalidate it for consistency, but the selector contract remains the four response-backed bands from the spec.
- The one-writer quota refresh rule must be cross-process or generation-fenced; an in-memory mutex is not enough because manual refresh and serve-owned refresh can run in separate processes. The post-`e7b55fc` executable choice remains state-backed SQLite lease persistence consumed by `codex-router-quota`; structural proof now permits state APIs to use `refresh_lease` vocabulary while forbidding the process-local secret-store lease in quota/CLI refresh-cycle entrypoints.
- Audit append failure policy for Plan 1 is best-effort with surfaced redacted
  local diagnostics for allowed proxy traffic. Local-auth failures still reject.
- Turn-state replay and previous-response ownership are no longer deferred to a
  T9 design packet. The post-`e7b55fc` executable choice is durable
  `codex-router-state` ownership with process-local replay requiring replan;
  selection turn-state/affinity code is codec-only or must be replaced.

## Artifacts Revised

- `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`

## Recommended Next Workflow

phase_result: `complete`

evidence:

- revised umbrella plan
- revised Plan 1A
- revised Plan 1B
- post-`918db95` validation-proof, codebase-boundary/execution, and
  scope-and-proof-fit planning lanes
- post-`e7b55fc` plan-review artifact and parent-verified plan revisions
- this plan revision ledger

recommended_next_workflow: `shravan-dev-workflow:plan-review-swarm`

recommended_transition_reason: The post-`e7b55fc` plan-review findings were folded into revised plan artifacts; the revised plans now need adversarial review before implementation starts.
