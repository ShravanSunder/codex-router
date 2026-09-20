# Host restart command — Specification

Governing needs: [Requirements U1–U6](requirements.md). This contract defines
observable Host and managed app-server lifecycle behavior; internal components
and call paths belong to Program Design.

## Observable command contract

### R1 / C1 — Whole-Host restart

When an owner invokes `codex-router host restart` against a compatible running
Host, the command MUST replace the whole Host with the installed `codex-router`
executable that issued the restart command, while preserving the running Host's
effective foreground arguments and configuration. It MUST NOT substitute the
older Host's captured executable path, accept an arbitrary replacement command,
or download or install a binary. Success MUST mean that the replacement Host has
reached the existing accepted local-readiness condition, including the existing
allowance for separately reported Remote Control degradation.

The replacement MUST preserve the effective foreground Host configuration,
router-owned state location, Codex home, configured endpoints, and child launch
policy. Host-owned children MUST be settled before replacement activates its
own generation. The stable singleton boundary MUST remain exclusively held so
that no second Host can acquire ownership during the handoff.

Basis: U1, U3, U4.

### R2 / C2 — Managed app-server restart

`codex-router host app-server restart` MUST restart the managed Codex app-server
without running an update and without replacing the Host. It MUST preserve the
existing graceful-shutdown, endpoint, readiness, Remote Control, and recovery-
budget semantics of the current app-server restart operation.

Basis: U2, U3.

### R3 / C3 — Managed app-server update

`codex-router host app-server update` MUST run the established managed Codex
update operation and activate the changed managed Codex executable when one is
installed. If no managed Codex change occurs, it MUST leave the current managed
runtime active and report no change. If updating fails before activation, it
MUST leave the current managed runtime active and report the failure. This
command makes no independent promise about the Host executable identity.

Basis: U2, U3, U4.

### R4 / C4 — Command cutover

The prior meanings of `codex-router host restart` as app-server-only restart
and `codex-router host update` as managed Codex update MUST be removed. Help and
usage output MUST identify the three distinct jobs in R1–R3. The existing
router-restart operation MUST retain its current behavior.

Basis: U2, U6.

## External context

```text
Owner/operator
  -- host restart / host app-server restart|update --> [codex-router Host]
  <-- terminal success, busy, or actionable failure -- [opaque system]

Installed codex-router executable -- activation input --> [opaque system]
Official managed Codex updater ---- update input ------> [opaque system]

Local Codex user  <-- established native endpoint ----- [opaque system]
Remote Codex user <-- established Remote Control result - [opaque system]

No public process-control surface, binary download service, or dashboard is added.
```

## Failure and compatibility behavior

- **No running Host:** each operator command MUST fail with the established
  actionable instruction to start `codex-router host`.
- **Concurrent mutation:** restart and app-server mutations MUST retain the
  existing single-mutation admission behavior and return a bounded busy result
  when another mutation owns the lifecycle boundary.
- **Failure before replacement starts:** `host restart` MUST report failure and
  MUST NOT claim that the installed Host was activated.
- **Failure after replacement starts:** the invoking CLI MUST use the existing
  bounded reconnect/readiness semantics to distinguish replacement readiness
  from replacement failure and provide the foreground-start recovery action on
  failure. Connection EOF alone MUST NOT be reported as success.
- **Interruption:** intentional Host or app-server replacement may disconnect
  clients. The app-server socket must accept and initialize again within the
  upstream Codex reconnect window (approximately 15 seconds), allowing the
  client to resume the same thread. Live connection continuity is not required.

### Shutdown and command shape

Managed app-server shutdown sends one SIGTERM, waits up to one second for exit,
then uses process-group SIGKILL as a backstop before bounded reap. Outcomes are
`Graceful`, `Killed`, and `TimedOutStillRunning`. The supported command
shape is `host`, `host status`, `host restart`, `host router restart`,
`host app-server restart`, and `host app-server update`.

### R5 / C5 — One-time legacy bootstrap

A Host released before the new restart request cannot satisfy R1. The release
documentation MUST give the one-time procedure
`stop foreground Host -> wait for exit -> start codex-router host`. The CLI MUST
NOT discover or signal a PID, silently reinterpret the request, or add a legacy
protocol shim.

Basis: U5, U6.

## Cross-cutting obligations

- The owner-private operator socket and singleton lock remain within the same
  trusted local-user boundary; restart MUST NOT introduce public process control.
- Restart and update MUST NOT reset or migrate router-owned state, Codex session
  state, credentials, or Remote Control pairing state.
- Whole-Host replacement MUST keep the existing bounded readiness deadline and
  terminal readiness classifications. No availability or latency SLA is added.
- The supported validation environment is an isolated debug Host with dedicated
  router-owned state and endpoints. Production Host replacement is prohibited
  for acceptance of this change.

## Requirement and proof coverage

```text
Need  Problem                                  Outcome                    Requirement  Contract  Proof
U1    P1 Host stays on the old image           O1 installed Host active  R1           C1        V1
U2    P2 lifecycle command names are ambiguous O2 distinct operator jobs R2-R4        C2-C4     V2
U3    P3 replacement can break live ownership  O3 bounded safe handoff   R1-R3        C1-C3     V1-V3
U4    P4 transport events can mask the result  O4 truthful terminal view R1,R3        C1,C3     V1,V3
U5    P5 old Host cannot decode new restart    O5 explicit bootstrap     R5           C5        V4
U6    P6 dual paths preserve ambiguity         O6 hard command cutover   R4,R5        C4,C5     V2,V4
```

- **V1 — Isolated cross-version Host replacement:** with two compatible candidate
  Host binaries that implement this restart contract built at distinct package
  versions, run A and install B at the same command path. Observe ordered child
  settlement, uninterrupted exclusive singleton ownership, activation of B,
  republished operator access, and the invoking CLI's terminal readiness or
  failure result. This does not claim transparent restart from the legacy
  protocol covered by V4.
- **V2 — CLI contract evidence:** command parsing and rendered help accept the
  R1–R3 spellings, reject the retired top-level meanings, and preserve explicit
  router restart behavior.
- **V3 — App-server lifecycle evidence:** in an isolated runtime, app-server
  restart performs no update or Host replacement; app-server update proves
  update failure, no change, changed activation, and replacement failure at the
  observable command boundary.
- **V4 — Bootstrap and prohibited-fallback evidence:** documentation inspection
  proves the one-time stop/wait/start procedure, while source and behavior
  evidence show no legacy request shim or PID discovery/signalling fallback.

The replacement proof MUST use distinct executable identities; a same-binary
re-exec cannot prove U1. Runtime evidence MUST use debug-only state and endpoints
and MUST NOT restart the production Host or production router.
