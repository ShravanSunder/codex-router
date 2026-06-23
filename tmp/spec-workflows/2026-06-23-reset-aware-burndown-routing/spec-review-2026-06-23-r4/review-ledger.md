# Reset-Aware Burn-Down Routing Spec Review Ledger R4

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `053d3069bad6596d202824c00768e74c1579fe50`
Coverage: 961 lines, read by parent in chunks 1-200, 201-400, 401-620, 621-820, 821-961
Verdict: needs revision

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage + progressive-disclosure | Harvey | answered | needs revision |
| requirements-testability + validation-and-testability + planning-readiness | Singer | answered | ready |
| contract-and-scope + architecture-boundaries + spec-difference | Popper | answered | needs revision |
| security-threat-model + WebSocket/protocol | Russell | answered | needs revision |
| adversarial-crux + guardrail-codification + UX/status | Faraday | answered | needs revision |

## What Held

- R3 `selected_next` blocker is mostly resolved: status uses neutral
  `preferred_next`, and runtime exact selection remains proxy-owned.
- Previous-response affinity is now state-owned, proxy-enforced, HTTP/SSE and
  WebSocket scoped, and fail-closed before weighted fallback.
- Live CLI smoke and WebSocket non-blocking proof expectations are explicit.
- Safe label and unsupported WebSocket path contracts are materially clearer.

## Accepted Findings

### R4-A1. Neutral `preferred_next` can still disagree on tie order

Severity: blocker

Required revision:

- Make `preferred_next` equal the winner of the exact neutral selector contract
  the proxy consumes.
- Emit and preserve the same ordered `weighted_candidates` list for assessment,
  status, and proxy selection.
- Replace prose-only tie rules with normative candidate ordering before
  `WeightedDeficitSelector`.

### R4-A2. `next use` lacks a same-pool non-preferred value

Severity: blocker

Required revision:

- Add `available` for accounts in the selected pool that are not
  `preferred_next`.
- Keep `held` for lower-priority pools only.
- Define exact derivation from availability, selected pool, and
  `preferred_next`.

### R4-A3. Public reason vocabulary is not closed

Severity: important

Required revision:

- Add public mapping from every assessment outcome to emitted
  `routing_reason`, human phrase, and `next use`.
- Cover `window_ineligible`, `window_exhausted`, `unknown_quota_window`,
  `missing_reset_time`, same-pool available, reserve held, and no-window refresh.

### R4-A4. V1 public long-window contract is ambiguous

Severity: important

Required revision:

- Choose v1 public UX as 5h plus weekly.
- Allow internal generic short/long helpers, but align requirements, status
  columns, enum names, and examples to 5h/weekly public wording.

### R4-A5. WebSocket current-state delta is still under-described

Severity: important

Required revision:

- Add current-state delta that current WebSocket code synthesizes a
  `/v1/responses` selection request and does not classify handshake path before
  selection.
- Name target shared route classification plus `unsupported_path` failure before
  selection, credential resolution, or upstream open.

### R4-A6. WebSocket first-frame guardrails need explicit bounds

Severity: blocker

Required revision:

- Freeze v1 first-frame resource contract: 1 MiB max frame, 250 ms wait bound,
  accepted type `response.create`, locally read fields limited to `type`,
  route-band metadata, and affinity metadata.
- State full request-schema validation remains upstream-owned.

### R4-A7. WebSocket redaction proof needs an explicit canary row

Severity: important

Required revision:

- Add WebSocket redaction proof with synthetic canary first-frame/request-body
  content and negative assertions against audit/log/smoke artifacts.

## Verdict

Needs revision. Do not route to `plan-creation-swarm` until these focused fixes
are folded in and the spec review gate reruns.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r4/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: R4 found focused vocabulary, preferred-next ordering, and WebSocket guardrail gaps that must be fixed before planning.
