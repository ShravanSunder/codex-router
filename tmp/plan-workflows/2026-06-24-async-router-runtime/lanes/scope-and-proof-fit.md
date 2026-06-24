# Lane: scope-and-proof-fit

Status: answered
Reasoning effort: high
Security context: applicable

## Accepted / Revised Slice Shapes

- Keep runtime substrate, but only as a shell/ownership checkpoint. It is not a
  completion slice while HTTP/SSE or WebSocket still route through blocking code.
- Keep SQLx async state/auth as a prerequisite slice and include credential
  refresh commit semantics. Do not expand into OAuth UX or quota redesign.
- Keep async HTTP/SSE as its own transport slice and prove mixed progress while
  WebSocket is stalled.
- Merge WebSocket transport with registry/revocation/observability for proof
  purposes. Treating cleanup as aftercare creates false-done risk.
- Split guardrails into early reachability inventory and final enforced command.
- Split installed-Codex proof into early real-serve smoke and final three-runtime
  e2e/soak orchestrator.

## False-Done Warnings

- WebSocket-only fix is out of scope compliance because HTTP/SSE is still
  blocking today and is explicitly in spec scope.
- Accept-loop-only fix is out; current accept already fans out threads.
- Release-linked blocking runtime is out, even as hidden compatibility path.
- Mock-only e2e is out for final acceptance.
- Hidden state/audit waits in pumps are out.
- Live OAuth/provider traffic is not default proof and needs separate approval.
- Session-picker, OAuth/login/keychain redesign, and quota algorithm redesign are
  out of this runtime goal.
- Tokenless loopback default must not drift.

## Missing Planning Inputs

- Exact command surface for release-path structural checker.
- Exact command surface for three-runtime concurrent e2e launcher.
- Exact command surface for five-minute soak artifact generator.
- Whether SQLx compile-time checked queries land in first state slice or later
  hardening. This does not block planning if the plan carries a decision row.

## Required Plan-Review Focus

- no generic "integration tests" rows
- no "WebSocket done" before mixed HTTP/SSE proof
- no final acceptance before real `codex-router serve` and three installed Codex
  runtimes
- guardrails check release graph / CLI contract, not only obvious files
- no OAuth UX, session-picker UX, or quota-policy redesign creep

## Completion Receipt

Reviewed slice fit against accepted spec, current runtime shape, smoke harness,
and false-done risks.

Confidence: high
