# Router Burndown, Quota Safety, and Sessions Goal

Date: 2026-06-25
Branch: followup-burndown-sessions-spec
Worktree: /Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.followup-burndown-sessions-spec

## Objective

Create the follow-up research, spec, and implementation plan for three product areas:

1. Quota burndown selection based on persisted one-week quota history, active run-rate, and active load.
2. Codex-safe quota exhaustion behavior so Codex does not see an account quota/usage-limit error unless every router account is exhausted.
3. Router-owned Codex session picker/list/last command that resumes through `codex --profile codex-router resume <SESSION_ID>`.

The added proof requirement is that quota, reset time, reset credits, rate-limit events, and quota exhaustion behavior must be proven with Codex-account-shaped end-to-end tests, with a separate live-gated proof path for real logged-in Codex accounts.

## Non-Goals

- No runtime implementation in this branch.
- No behavior beyond account routing, credential selection, quota safety, and pass-through compatibility.
- No mid-stream WebSocket payload mutation or per-message account hot-swap.
- No transcript-content search in the sessions command.
- No ratatui/dialoguer/direct crossterm dependency for the router sessions V1.

## Workflow State

- orchestrator-goal: active for spec/plan creation.
- research-swarm: completed with three read-only lanes.
- spec-creation-swarm: completed by parent synthesis into the spec artifact.
- plan-creation-swarm: completed by parent synthesis into the plan artifact.

## Artifacts

- Research ledger: tmp/research-workflows/2026-06-25-router-burndown-sessions/research-ledger.md
- Spec: tmp/spec-workflows/2026-06-25-router-burndown-sessions/router-burndown-quota-safety-sessions-spec.md
- Plan: tmp/plan-workflows/2026-06-25-router-burndown-sessions/implementation-plan.md

