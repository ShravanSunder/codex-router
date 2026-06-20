# Codex Router Plan Review

Date: 2026-06-20
Workflow: `shravan-dev-workflow:plan-review-swarm`
Target: `docs/plans/2026-06-20-codex-router-implementation-plan.md`

## Coverage

- Reviewed plan line coverage after revisions: 861 lines.
- Reviewed spec line coverage after revisions: 450 lines.
- Reviewed workflow state: `tmp/workflow-state/2026-06-20-codex-router/details.md`
  and `tmp/workflow-state/2026-06-20-codex-router/events.jsonl`.
- Subagent lanes requested and used: spec compliance, architecture assumptions,
  testability validation, security/reliability, execution scope, adversarial
  design.

## Accepted Findings Applied

1. Source provenance was not strong enough as the first implementation gate.
   Added R0/T0 source-preflight requirements for current Codex source, Prodex
   source, official manual evidence, installed Codex version, and smoke command
   shape before scaffolding.
2. Installed-Codex smoke used an invalid command shape. Replaced profile-based
   `codex debug models` claims with runtime smoke using
   `codex --profile codex-router exec ...`, and required any debug/model probe
   to use separately verified overrides or protocol mocks.
3. The fake `codex-router live-proof ...` placeholder implied a CLI that does
   not exist. T11 is now runbook-only unless a live-proof CLI is explicitly
   designed, implemented, and tested.
4. Host bootstrap commands mutate `~/.rustup` and `~/.cargo`. T0/T1 now require
   explicit approval before installing Rust toolchains or cargo tools, and the
   plan stops before product code if approval is withheld.
5. The durable state substrate was unresolved. The plan now specifies
   `codex-router-state` with SQLite metadata, migrations, repository traits,
   corruption tests, and no OAuth token storage in SQLite.
6. Local router auth depended on secret storage but was ordered before it.
   T3/T4 were reordered so the secret-store backend and refresh leases exist
   before local auth uses them.
7. Quota was too close to credential material. The plan now requires quota code
   to use auth-owned account/quota facades and state repository traits, with a
   guard that `codex-router-quota` has no normal/build dependency on
   `codex-router-secret-store`.
8. Proxy integration could have started before contracts were stable. Added
   T7.5 to freeze facade contracts before T8, including authenticated quota,
   state repositories, selection decisions, reservation handles, turn-state
   envelopes, and precommit failure classification.
9. Route classification ownership was muddled. The proxy now owns route
   classification and protocol parsing; core owns only stable route-kind/value
   types.
10. Loopback-only behavior needed a concrete proof gate. Added R4A plus
    config/proxy tests for rejecting non-loopback listeners before startup.
11. WebSocket header safety needed direct proof. Added R11A to prove local auth,
    client-supplied upstream auth, cookies, and hop-by-hop headers are stripped
    before selected upstream auth is injected exactly once.
12. WebSocket pre-selection needed hostile-frame bounds. Added R11B for missing,
    binary, malformed, non-`response.create`, oversized first frames, and bounded
    pre-selection waits.
13. Corrupt persisted state was under-specified. Added R18A so corrupt or
    partially persisted auth/quota/account/affinity state fails closed for
    affected accounts while keeping healthy accounts eligible.
14. Audit sink defaults were not concrete. The default is now a private
    router-root file sink with allowlisted, redacted event serialization.
15. WebSocket smoke could have been skipped silently. T10 now requires installed
    Codex smoke to assert at least one WebSocket handshake when the profile has
    `supports_websockets = true`; if installed Codex does not choose WebSocket,
    implementation must stop and replan.
16. Product non-goal guards were too broad or too weak. R23 now uses targeted
    forbidden patterns for Prodex provider-core/gateway/admin/context/session
    repair scope instead of raw broad terms.
17. CI YAML proof was missing. Added `actionlint .github/workflows/ci.yml`.
18. PR readiness was impossible to prove without a remote. T12 now keeps PR
    readiness as the terminal goal but treats missing remote/GitHub PR setup as
    an external authorization blocker, not as completion.
19. Activation proof was still under-specified. R19/T4/T9/T10 now require exact
    profile content assertions, a shell-safe token export command, and a smoke
    harness that consumes helper-produced profile/env output instead of
    hand-built fixtures.
20. `actionlint` was required but not reproducible from bootstrap instructions.
    T0/T1 now check and install `actionlint` via explicit host-bootstrap
    approval, with `brew install actionlint` as the planned command.
21. External Codex/Prodex provenance refresh could have mutated source checkouts
    silently. T0 now keeps external checkout verification read-only unless a
    provenance refresh mutation is explicitly approved.

## Rejected Or Deferred Findings

1. Rejected narrowing the terminal condition to local-only proof. The goal still
   requires PR readiness by default; the plan now names missing remote/PR setup
   as an external authorization blocker if it has not been authorized by T12.
2. Deferred macOS Keychain and 1Password secret backends. The first backend is a
   hardened file store because it is deterministic and locally provable; other
   backends remain future adapters behind the same trait.
3. Deferred live OAuth/quota execution. Mock OAuth/token/quota proof is required
   during implementation; real account proof remains gated by explicit approval
   and redacted runbook commands.

## Post-Review Plan State

- Requirement/proof matrix now has 29 requirement rows and no duplicate IDs.
- Current routing state-machine proof covers local auth before selection,
  HTTP/SSE preservation, WebSocket first-frame selection, connection-scoped
  WebSocket affinity, turn-state replay protection, previous-response affinity,
  quota-aware weighted deficit selection, and precommit-only rotation.
- Current Rust setup requirements include `rust-toolchain.toml`, workspace
  lint policy, rustfmt policy, deny/audit config, GitHub Actions, actionlint,
  cargo-nextest, cargo-deny, and cargo-audit gates.
- Current activation requirements prove the generated Codex profile contract and
  shell-safe local token export path before installed-Codex smoke is allowed to
  pass.

## Verdict

The plan is ready for `shravan-dev-workflow:implementation-execute-plan`, with
the first execution checkpoint being T0 source provenance and host-bootstrap
approval detection. No product code should be scaffolded until T0 confirms
current source/docs evidence and either obtains approval for host toolchain
mutation or reports that setup blocker.

phase_result: complete
evidence: revised implementation plan, revised spec smoke wording, this review
report, stale-command search, proof-matrix ID check, `git diff --check`, and
workflow-state JSON validation
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Plan-review findings have been applied; the next
unproven lifecycle gate is implementation execution starting with T0 source
provenance and host-bootstrap approval detection.
