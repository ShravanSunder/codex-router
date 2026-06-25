# Router Burndown Sessions Implementation Goal

Date: 2026-06-25
Branch: impl-burndown-sessions
Worktree: /Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.impl-burndown-sessions

## Objective

Implement the reviewed quota burndown, Codex-safe quota exhaustion, quota proof, and sessions plan with TDD and pyramid validation.

## Source Artifacts

- Spec: tmp/spec-workflows/2026-06-25-router-burndown-sessions/router-burndown-quota-safety-sessions-spec.md
- Plan: tmp/plan-workflows/2026-06-25-router-burndown-sessions/implementation-plan.md
- Spec review cycle 1: tmp/spec-workflows/2026-06-25-router-burndown-sessions/spec-review-cycle-1/review-report.md
- Plan review cycle 1: tmp/plan-workflows/2026-06-25-router-burndown-sessions/plan-review-cycle-1/review-report.md

## Hard Constraints

- SQL access for new/extended implementation work is SQLx only.
- Do not add or extend rusqlite queries, repository traits, migrations, session readers, or test helpers.
- No mid-stream WebSocket account hot-swap.
- Router remains an account router; normal payload behavior is pass-through except narrow quota/auth/account-routing observations.
- Use TDD red/green for behavior changes.

## Current Workflow

Current workflow: shravan-dev-workflow:implementation-execute-plan
Next workflow: shravan-dev-workflow:implementation-review-swarm

