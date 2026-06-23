# Quota Burn-Down Routing Plan Review Ledger

Date: 2026-06-23
Status: needs revision findings folded into spec and plan

## Reviewed Artifacts

- `tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`
- `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- live code anchors in `crates/codex-router-selection`,
  `crates/codex-router-proxy`, `crates/codex-router-cli`,
  `crates/codex-router-state`, and `crates/codex-router-test-support`

Coverage after parent revision:

- spec: 1234 lines
- plan: 406 lines

## Lanes

- whole-plan-cohesion + spec-compliance:
  `tmp/plan-workflows/2026-06-23-quota-burndown-routing/plan-review/lanes/whole-plan-cohesion.md`
- architecture-assumptions + execution-scope:
  `tmp/plan-workflows/2026-06-23-quota-burndown-routing/plan-review/lanes/architecture-execution.md`
- testability-validation + security-reliability:
  `tmp/plan-workflows/2026-06-23-quota-burndown-routing/plan-review/lanes/validation-security.md`

## Parent Verdict

Initial lane verdict was `needs revision`.

Accepted findings were folded into the source spec and implementation plan in
the same review pass. The plan can proceed to implementation validation only
after T0 proves the dirty target-file gate is satisfied.

## Accepted Findings And Resolution

1. Probe scheduling ownership was ambiguous.
   Resolution: v1 is narrowed to prompt startup and periodic background
   refresh/probe only. Request handling does not create a provider call,
   synchronous trigger, or proxy-to-worker probe queue.

2. Unknown/no-data probe behavior needed stricter proof.
   Resolution: spec and plan now require fail-fast request behavior from
   persisted SQLite state, no request-path provider I/O, and later promotion
   only after background refresh/probe persists verified selector rows.

3. WebSocket proof was too weak.
   Resolution: T3 now requires the full preselection failure matrix, zero
   selector advance, zero credential resolution, zero upstream auth injection,
   zero upstream open, first-frame allowlist canaries, redaction canaries, and
   delayed/failing-refresh non-blocking proof.

4. Status smoke proof was missing.
   Resolution: T4 now requires a repo-owned live CLI smoke for
   `quota status --format table|plain|json` over persisted fixtures, with
   negative output assertions.

5. Installed Codex e2e did not force reset-aware cross-surface agreement.
   Resolution: T6 now requires a multi-account fixture, forced winner,
   quota-status agreement, selected safe label/hash, routing reason, and
   WebSocket multi-turn pinning proof.

6. Previous-response affinity was treated as existing behavior.
   Resolution: T2/T3 now make affinity extraction/enforcement explicit
   implementation work using the existing `AffinityRepository` state contract.

7. OAuth account switching cooldown was missing.
   Resolution: spec now defines a route-band account-hold cooldown with a
   default v1 120 second hold. Plan T2/T3/T6 require tests proving reuse inside
   the hold window and immediate break for affinity, exhaustion, blocked,
   disabled, credential-invalid, or probe-required states.

8. Dirty target files overlapped planned write scopes.
   Resolution: T0 is now a hard stop gate. Before implementation, target files
   must be clean or each overlapping dirty file must be explicitly adopted.

9. CLI status could drift from runtime math.
   Resolution: lane C is gated on lane A public DTO/API freeze. CLI must import
   `codex-router-selection::burn_down` types instead of reimplementing math.

10. Validation commands were not complete.
    Resolution: validation now includes the installed Codex smoke wrapper, a
    planned quota-status smoke wrapper, and workspace clippy with `-D warnings`.

## Rejected Or Deferred Findings

- A new durable request-triggered probe queue/status owner was not accepted for
  this slice. It conflicts with the user's background-only request-path
  correction and would broaden the architecture. Future on-demand probe
  scheduling should be a separate lower-layer state/queue design.

## Next Route

Recommended next workflow:
`shravan-dev-workflow:implementation-execute-plan`

Required first action in execution:
run the T0 dirty target-file gate and stop if planned target files are neither
clean nor explicitly adopted.

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-23-quota-burndown-routing/implementation-plan.md`, `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Plan-review blockers were accepted and folded into the spec and plan; implementation may start only after T0 validates the dirty target-file gate.
