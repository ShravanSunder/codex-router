# Spec Creation Ledger

Goal id: `2026-06-25-codex-router-account-router-ux-fix`

Primary spec: `tmp/spec-workflows/2026-06-25-codex-router-account-router-ux-fix/account-router-ux-fix-spec.md`

## Source Inputs

- Goal details: `tmp/workflow-state/2026-06-25-codex-router-account-router-ux-fix/details.md`
- Review packet: `tmp/implementation-review-workflows/2026-06-25-account-router-law-review/review-packet.md`
- Parent findings: `tmp/implementation-review-workflows/2026-06-25-account-router-law-review/parent-findings-so-far.md`
- Async runtime spec: `tmp/spec-workflows/2026-06-24-async-router-runtime/async-router-runtime-spec.md`
- Reset-aware burndown spec: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- Async runtime plan proof matrix: `tmp/plan-workflows/2026-06-24-async-router-runtime/implementation-plan.md`
- Burndown/sessions plan: `tmp/plan-workflows/2026-06-25-router-burndown-sessions/implementation-plan.md`

## Current-State Evidence Accepted

- Release WebSocket path constructs `FirstFramePolicy::new(1024 * 1024)`.
- Release WebSocket path exposes `FirstFrameTooLarge`, `MalformedFirstFrame`, and whole-frame JSON parsing before upstream open.
- The older reset-aware spec contains now-rejected first-frame size/shape rules; the new spec hard-cuts those rules.
- CLI still uses a hand parser for normal command surfaces except the sessions subcommand.
- Workspace dependencies already include `clap`, `inquire`, `comfy-table`, `sqlx`, Hyper, `hyper-tungstenite`, and `tokio-tungstenite`; `indicatif` is not currently present.
- `quota status` currently computes a persisted SQLite quota assessment and labels the result as `next`, which can diverge from live router selection with active reservations/holds.
- A drifted local DB had `active_client_leases` under `user_version=7`; current source does not define that table at version 7.

## Lane Policy

The parent used existing gpt-5.5 implementation review lanes as current-state evidence rather than launching a second large discovery swarm. This honors the user's instruction to avoid repeated review cycles.

Selected spec-creation lanes covered locally by parent synthesis:

- codebase-explorer
- ux-api-cli-surface
- security-trust-boundary
- architecture-minimal
- architecture-clean-boundary
- architecture-pragmatic
- risk-and-tradeoff-design

## Accepted Decisions

- Active load means active turn, not socket lifetime.
- Re-reserving load on later same-socket request-like frames is required; account switching is still not allowed within the WebSocket connection.
- `quota status` cannot call static persisted prediction live `next` unless live router state is actually included.
- Normal CLI help must be cleaned through clap-owned command contracts.
- `account login` defaults to device auth.
- Internal/test/live/proof commands are hidden or moved to advanced surfaces.
- `sessions` default root filter is cwd; `--checkout`, `--repo`, and `--any` replace generic `--scope` as the normal UX.
- SQLx is required for new/extended SQLite work.

## Contested Or Open

- Whether live quota diagnostics are hidden or removed. Default spec answer:
  hidden/advanced unless removal is cheaper during clap migration.
- Whether live active-turn diagnostics need a control endpoint or can reuse
  existing report machinery. Default spec answer: do not invent a full admin API
  unless implementation proof needs it.
- Whether token commands remain hidden or advanced. Default spec answer: hidden
  or advanced because tokenless is normal mode.

## Spec Review Cycle 1

Reviewer: `019f0180-cf44-7f11-8c7a-302482d8ddc3`

Result: `needs_revision`, then addressed in the primary spec.

Accepted findings addressed:

- Defined bounded top-level auth-smuggling detector contract.
- Defined pass-through-safe active-turn re-reservation state machine.
- Defined normal login credential storage policy and plaintext-file constraint.

Accepted to plan:

- Current-head proof rows and freshness guards.
- Hide or advance `quota status`, token/live/proof flags, and import flows while
  normal help teaches the cleaned contract.
- Treat process-local active-turn tracking as default; add persisted/live
  diagnostics only if implementation planning proves they are needed.

## Completion Receipt

phase_result: complete
evidence: `tmp/spec-workflows/2026-06-25-codex-router-account-router-ux-fix/account-router-ux-fix-spec.md`
recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`
recommended_transition_reason: One spec review cycle was run and accepted findings were folded into the spec; the contract is ready for implementation planning.
