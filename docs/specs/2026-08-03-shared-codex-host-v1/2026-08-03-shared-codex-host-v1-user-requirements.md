# Shared Codex Host V1 — User Requirements

Date: 2026-08-03

This is the governing user-requirements source for Shared Codex Host V1. The
July 2026 host documents are prior attempts and may supply observations, but
they do not authorize requirements for this design.

## User and stakeholder classes

- **Owner/operator:** one trusted logged-in owner of a personal Mac who starts,
  observes, restarts, and updates the local Codex environment.
- **Local Codex user:** the same owner using interactive Codex CLI or Codex
  Desktop against the shared local app-server.
- **Remote Codex user:** the same owner using Codex Remote Control against that
  app-server.
- **Excluded classes:** other local users, fleets of Macs, partially trusted or
  public clients, and downstream products requiring a new Codex protocol.

## Goal boundary

The owner wants one small `codex-router host` operating surface that keeps the
existing router and one upstream Codex app-server usable together. Local Codex
clients connect directly to the app-server's conventional Unix socket; the
host is never a client-traffic proxy. `codex-router serve` remains the model
router and `codex-router sessions` remains session discovery and launch.

The missing behavior is explicit shared-app-server startup, bounded crash
recovery, Remote Control enablement, session attachment, status, and a manual
update operation that restarts the app-server only when Codex actually changed.

The design must reuse upstream Codex lifecycle, socket, Remote Control, thread,
turn, approval, persistence, and graceful-shutdown behavior. It may add a small
host command/runtime inside this repository. Adding launchd ownership, client
traffic proxying, replacement Codex APIs, thread or connection databases,
thread or connection polling, automatic update polling, multi-generation
handoff, fleet management, or a general lifecycle platform exceeds the V1
complexity budget and requires a new owner decision.

## Requirements

| ID | Affected class | Need or outcome | Evidence and authority | Priority |
| --- | --- | --- | --- | --- |
| U1 | Owner/operator | I can launch one local shared Codex environment through `codex-router host`; it uses the existing `codex-router serve` model router and one upstream app-server. | Owner-authorized in the 2026-08-02 to 2026-08-03 design conversation. | Must — owner |
| U2 | Local Codex user | CLI and Desktop connect directly to the same app-server through its stable native Unix socket. The host is not in their protocol or data path. | Owner-authorized correction rejecting a host proxy and selecting direct socket attachment. | Must — owner |
| U3 | Local and remote Codex user | Model traffic produced by the managed app-server uses the local `codex-router`; loss of the router is visible rather than silently treated as a healthy hosted environment. | Owner-authorized statement that the app-server talks to `codex-router`. | Must — owner |
| U4 | Local Codex user | `codex-router sessions` remains the session picker/launcher and starts or resumes interactive work against the shared app-server. | Owner-authorized command-boundary correction. | Must — owner |
| U5 | Remote Codex user | Remote Control is enabled on the same managed app-server and returns after an ordinary restart without a router-owned pairing or relay implementation. | Owner-authorized requirement to start app-server with Remote Control; upstream Codex retains ownership. | Must — owner |
| U6 | Owner/operator | I can launch the host manually and invoke explicit app-server restart, update, status, and router-restart operations without launchd in V1. Restart uses upstream Codex's native Unix graceful restart behavior. Stopping the foreground host is an ordinary CLI cancellation, not a separate service-manager product. | Owner-authorized manual-operation boundary and command discussions. | Must — owner |
| U7 | Owner/operator and connected clients | `codex-router host update` runs the official Codex updater first. If Codex did not change, the running host, app-server, and clients are untouched. If Codex changed, the current host gracefully stops its children and restarts itself; the replacement host starts the updated app-server on the same socket. If updating fails, the current host and app-server keep running. | Explicitly owner-confirmed on 2026-08-03 and clarified as a whole-host restart on 2026-08-04. | Must — owner |
| U8 | Owner/operator | While the manually launched host is in steady operation, one ordinary unexpected app-server exit receives one restart attempt; another failure is reported and left for manual recovery. A failure during an explicit launch, restart, update, or stop belongs to that operation and does not start nested recovery. Host-process death itself has no V1 continuity guarantee. | Owner selected the bounded one-restart option and rejected combinatorial lifecycle handling for V1. | Must — owner |
| U9 | Owner/operator | Status and OpenTelemetry make router, app-server, version drift, update, and bounded recovery outcomes understandable without exposing prompts, credentials, or protocol payloads. Existing Victoria ingestion is sufficient; no dashboard or metrics warehouse is required. | Owner-authorized observability boundary. | Should — owner |
| U10 | All in-scope classes | V1 requires no upstream Codex changes or fork and does not create a custom Codex protocol, client-connection registry, thread/session database, connection/thread polling loop, multi-generation handoff, automatic updater, launchd service, or cross-Mac control plane. | Repeated explicit owner boundary corrections, including the decision not to infer drain safety by polling Codex state. | Must — owner |

All rows are `authorized` and therefore eligible to govern the specification.
Code and upstream documentation remain observational evidence for feasibility;
they do not add requirements.

## Owner journey inputs

The material owner sequence is:

1. launch the host from the CLI;
2. use CLI, Desktop, or Remote Control against the one app-server;
3. inspect status or recover one ordinary app-server crash;
4. explicitly request an update;
5. remain connected when no update exists, or reconnect after a real update.

The current pain is separate unmanaged launches and uncertain update/restart
behavior. The desired difference is one bounded operating surface whose update
and interruption behavior is predictable. These steps cite U1 through U9.

## Evidence gaps that do not change product meaning

- Exact installed Codex CLI and Desktop attachment behavior must be proved for
  the selected upstream release.
- Exact app-server daemon and Remote Control commands are version-bounded
  because upstream marks the daemon experimental.
- The implementation must prove the app-server's effective model-provider path
  reaches `codex-router`; this requirement does not authorize an upstream Codex
  change or a new enforcement proxy.

If any of these facts makes U1 through U10 infeasible without exceeding the
complexity budget, design returns to the owner rather than weakening the row.
