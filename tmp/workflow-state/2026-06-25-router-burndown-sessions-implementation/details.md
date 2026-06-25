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

Current workflow: shravan-dev-workflow:implementation-pr-wrapup
Next workflow: terminal after PR checks are green, comments/review threads remain clear, mergeability is clean, and final quiet-poll re-fetch passes

## Latest PR Wrap-up State

- Branch pushed: impl-burndown-sessions at 47b238b1762eca8224d966ab05325997845dbafa
- PR: https://github.com/ShravanSunder/codex-router/pull/3
- Local proof: fmt, clippy, workspace nextest, build, quota smoke, installed-Codex serial, concurrent WebSocket, and soak passed.
- GitHub state: PR head matches local HEAD; no PR comments, reviews, or review threads; Workflow lint check succeeded; Rust check remains in progress as of 2026-06-25T11:44:09-04:00.
- Completion status: not complete until Rust check finishes green and a quiet-poll/final re-fetch confirms readiness.
