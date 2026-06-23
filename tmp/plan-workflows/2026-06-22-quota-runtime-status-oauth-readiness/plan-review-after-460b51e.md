# Plan Review After `460b51e`

Reviewed commit: `460b51ec1c85dac3bbb0392ed72d78584740ff04`

Verdict: `needs_revision`

The revised plans are closer, but they are not ready for
`implementation-execute-plan`. Five read-only review lanes completed, and all
returned `needs_revision`.

## Coverage

- Umbrella plan: 323 lines
- Plan 1A: 419 lines
- Plan 1B: 488 lines
- Source spec: 497 working-tree lines
- Prior review receipt: 184 lines
- Revision ledger: 82 lines

## Accepted Blocker Findings

1. Source artifact freeze is not reproducible from `460b51e`.
   - `HEAD` contains a 455-line spec, while the reviewed working-tree spec has
     497 lines.
   - Required revision: Gate 0 must either commit/promote dirty source inputs
     before execution or carry them forward with path, source commit/head,
     checksum or byte count, and normative/non-normative flag.

2. Credential atomicity is still an implementation fork.
   - Plan 1A T3 offered two storage contracts.
   - Required revision: choose one concrete credential storage/read contract,
     name API ownership, and make T4 depend on it.

3. Structural proof rows are not copy-paste runnable.
   - Rows such as `1A-04b`, `1A-14`, and `1A-14a` use `same command`.
   - Required revision: write exact commands, including negative `rg` commands
     that fail on forbidden matches.

4. Installed smoke can still be marked `not-run`.
   - Final closeout lets `1B-23` through `1B-26` be `not-run`, but only live
     OAuth/quota proof is approval-gated.
   - Required revision: rows `1B-23` through `1B-25` must pass; only `1B-26`
     may be `not-run: approval required`.

5. Installed smoke rows do not explicitly prove startup-not-quota-blocked or
   redacted status-table capture.
   - Required revision: add or expand exact installed-smoke rows to include
     startup/status observations.

6. Resolver-bypass proof is scoped too narrowly for final runtime shape.
   - Plan 1B moves quota runtime into `codex-router-quota`, but final bypass
     proof does not include that crate and all runtime egress surfaces.
   - Required revision: add a Plan 1B/final resolver-bypass row over quota,
     proxy, CLI serve/refresh, and bootstrap/file-store boundaries.

7. Local bearer lifecycle proof misses proxy-level old/wrong HTTP-token
   rejection.
   - Required revision: keep the core token-classifier row narrow and add a
     proxy-level row for empty/wrong/old HTTP token rejection before selection
     or upstream.

## Accepted Important Findings

1. Plan 1A T1 proof is too weak for account/quota/serve extraction.
   - Add exact account/quota/serve regression rows or a small regression set.

2. Plan 1A validation omits CLI and proxy package gates, even though Plan 1A
   edits those packages.

3. Selector and credential resolution boundaries remain entangled.
   - Add target call order: selector returns account/route decision without raw
     token material; auth resolver returns provider auth immediately before
     upstream egress.

4. Family-atomic publication needs an explicit state API cutover.
   - Add a state-owned `replace_response_family_quota_state`-style API or
     named generation-fence schema and a real SQLite proof row.

5. Replay-safe affinity lacks a named replay-state owner, TTL, restart behavior,
   and durable/process-local semantics.

6. Audit append-failure diagnostics do not name the diagnostic channel.

7. Some existing-test rows overclaim:
   - `1B-17a` is classifier-only.
   - `1B-17b` does not prove query/body semantics beyond its current fixture.
   - `1B-18` overclaims status table field coverage.
   - `1B-17e` mixes a test-shaped row with a not-applicable escape.

## Rejected Or Non-Blocking Notes

- Plan 1A/1B serial sequencing and checkpoint discipline are now materially
  enforceable.
- Workflow state is internally consistent for transition to plan review at
  `460b51e`.
- Cross-process refresh lease proof is directionally adequate; tighten wording
  to independent SQLite connections/processes with no shared memory.
- WebSocket first-frame behavior is covered by `1B-15`.
- Live-proof gating is covered for `1B-26`.

phase_result: needs_revision
evidence: commit 460b51ec1c85dac3bbb0392ed72d78584740ff04; five read-only plan-review lanes; this parent receipt
recommended_next_workflow: shravan-dev-workflow:plan-creation-swarm
recommended_transition_reason: The plan still needs revisions for source-freeze reproducibility, credential atomicity, exact structural commands, mandatory installed smoke, resolver-bypass scope, proxy-level local-auth proof, selector/resolver separation, family-atomic state API, replay-state ownership, audit diagnostics, and overclaimed existing-test rows.
