# Host restart command — Requirements

## Purpose and boundary

A trusted owner who installs a newer `codex-router` needs one direct command to
activate that installed Host binary. Before this change, `codex-router host restart` restarts
only the managed Codex app-server, so the Host can remain on an older process
image while reporting healthy app-server status.

This change separates whole-Host lifecycle from managed app-server lifecycle.
It covers the foreground Host, its operator CLI, the installed `codex-router`
executable, Host-owned children, singleton ownership, and terminal readiness.
It does not add a router download/update operation or a background service.

## Consumers

- **Owner/operator:** installs `codex-router` and invokes Host lifecycle commands.
- **Local and Remote Codex users:** may be interrupted while the Host or managed
  app-server is intentionally replaced, and need the resulting service to
  return to its established endpoints and readiness contract.

## Authorized needs

All rows are required. Authority is the owner's confirmed Host restart decisions
on 2026-09-13. The command contracts below are self-contained; disposable
investigation and review logs are not part of this requirements authority.

| ID | Affected class | Need and reason |
| --- | --- | --- |
| U1 | Owner/operator | Replace the entire running Host with the latest already-installed `codex-router`, so installation can be activated without coupling it to a managed Codex update. |
| U2 | Owner/operator | Give whole-Host restart, app-server restart, and app-server update distinct command names and observable jobs. |
| U3 | Owner/operator; Codex users | Preserve effective Host configuration, exclusive singleton ownership, owned-child lifecycle, established endpoints, and bounded readiness reporting across whole-Host replacement. |
| U4 | Owner/operator | Receive a terminal success, busy, or actionable failure result instead of mistaking request delivery or connection loss for completed replacement. |
| U5 | Owner/operator | Have an explicit one-time recovery path when the already-running Host predates the new restart protocol and cannot understand the request. |
| U6 | Maintainers | Make the command cutover direct: retire the ambiguous top-level app-server spellings without a compatibility shim or process-ID signalling fallback. |

## Operator journey

```text
Install a newer codex-router (U1)
  -> invoke `codex-router host restart`
  -> current pain: only the app-server restarts; the Host image stays old
  -> desired difference: the installed Host becomes active and the command
     reports bounded replacement readiness or an actionable failure (U3, U4)

First upgrade from an older protocol (U5)
  -> stop the foreground Host in its owning terminal
  -> wait for that Host to exit and release ownership
  -> start `codex-router host`
```

## Limits and non-goals

- `host restart` does not download or install `codex-router`.
- Clients must reconnect and initialize the resumed thread within the upstream
  Codex reconnect window (approximately 15 seconds) after an intentional
  replacement; uninterrupted TCP continuity is not promised.
- Host shutdown sends one SIGTERM, waits up to one second for exit, then uses
  process-group SIGKILL as a backstop before bounded reap. The one-second
  boundary is shared by Host-owned app-server replacement paths so clients can
  reconnect within the upstream window.
- No automatic rollback, PID discovery/signalling, legacy request shim, status
  dashboard, Host binary-drift status, or production process replacement is in
  scope.
- The existing explicit router-restart capability remains unless the command
  hierarchy mechanically requires a spelling adjustment; its behavior does
  not change.
- Validation uses an isolated debug runtime. It must not stop, restart, or
  replace the production Codex router or production Host.
