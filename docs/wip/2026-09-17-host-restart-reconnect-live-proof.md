# Isolated live-proof status

Residual proof notes: process-lifecycle fixtures cover the `Killed` outcome and
process-group escalation, and the runtime restart integration now observes the
`AppServerKilled` progress frame over the operator socket. A resumed
`TimedOutStillRunning` shutdown reissues the process-group KILL backstop.

Re-exec timing comparison: the pre-instrumentation live sample reported
`re-executing Host (4.62s)` to router readiness. The post-instrumentation
compiled acceptance recorded profile/spec projection 0 ms, router-root setup
1 ms, fixture launchctl policy 239 ms, singleton acquisition 243 ms, and
executable identity 0 ms. The measured re-exec span remains approximately
4.6 s; no safe reduction was claimed because the managed executable identity
and version are required inputs to construct the app-server launch plan before
spawn. Production launchctl timing was not exercised.

The fixture-backed compiled CLI acceptance is green, including whole-Host
replacement from a new install path. A direct isolated debug Host was then
launched with fresh `/private/tmp` router and socket roots and port `18787`.

An initial attempt using a temporary `CODEX_HOME` was rejected because the
debug profile is owner-managed. A retry using the readable owner profile and
fresh isolated router/socket roots succeeded. The first sandboxed retry also
hit the local-network policy; the approved isolated launch then reached:

```text
✓ router ready
listening: 127.0.0.1:18787
```

Three-restart timing run (2026-09-17, isolated debug Host, port 18787):

- Restart 1 CLI wall time: 6.710 s; `re-executing Host`: 4.60 s.
- Restart 2 CLI wall time: 6.599 s; `re-executing Host`: 4.55 s.
- Restart 3 CLI wall time: 6.675 s; `re-executing Host`: 4.57 s.

Representative restart 1 Host timeline, with new-image process start at
`22:22:40.324331Z` as offset zero:

```text
00.000  profile/spec projection complete
00.000  router-root preparation complete
00.001  singleton acquired
00.415  executable identity complete
00.424  managed executable version complete
00.424  app-server launch plan built
00.425  app-server spawn issued
00.921  router ready
01.612  app-server socket/native readiness complete
01.737  public readiness observation began
02.135  public schema verified
03.818  public validators resolved
03.818  public generation published; AwaitHostStart can converge
```

The old CLI began restart 1 at `22:22:37.300Z` and completed at
`22:22:44.300Z`. Approximately 3.0 s therefore elapsed between the old
Host's `ReExecuting` phase and the new image's first timing record. The
app-server socket was ready about 1.2 s after new-image start, well before the
CLI completed the re-exec observation. The unexplained time is in the old
Host's pre-exec handoff (telemetry flush plus socket/lock transition), not
router readiness, app-server spawn, CLI retry cadence, or presenter phase
closure. Telemetry initialization is not separately timestamped, so no finer
sub-attribution is claimed.

Post-fix timing run after moving telemetry flush ahead of child teardown
(2026-09-17 22:30Z) produced three wall-clock CLI gaps of approximately
`6.667 s`, `6.724 s`, and `6.903 s`; presenter `re-executing Host` spans were
`4.45 s`, `4.58 s`, and `4.63 s`. The new-image records show router readiness
at roughly `11 ms` and app-server socket/native readiness at `1.12–1.26 s`
after process start. Public generation publication completed at about
`2.06–2.14 s`. The remaining multi-second client gap is therefore dominated by
the CLI's bounded AwaitHostStart reconnect cadence waiting for the operator
socket/public generation, not by app-server socket acceptance. The pre-exec
telemetry flush is now outside the child-teardown-to-exec window; no additional
safe change was made at this boundary.

Final teardown-to-exec instrumentation (2026-09-17 22:33Z) recorded:

```text
preExecTelemetryDone  +5 ms from activation start
appServerStopped      +9 ms
reExecutingQueued     +9 ms
reExecutingAcked      +0 ms (100 ms bound)
operatorSocketRemoved +0 ms
lockPrepared          +0 ms
new-image first record 22:33:03.035058Z
```

The old app-server exit to lock preparation was approximately 0 ms after the
9 ms teardown point; the replacement image's first instrumented record arrived
about 371 ms later. This isolates the remaining interval to process image
startup before `foreground_launch` timing begins (debug-build cold start and
telemetry initialization). The app-server socket then reached native readiness
at `+1.16 s` from the new-image timing start.

No production process, port `8787`, or `~/.codex-router` state was modified.
These are verbatim non-TTY captures from the isolated debug Host. The
installed/production launchctl policy path was not exercised.

```text
$ codex-router host app-server restart --port 18787 --router-root /private/tmp/host-reconnect-live.E9yJd1/router
✓ preparing app-server (1.57s)
✓ stopping app-server (3.71ms)
✓ starting app-server (68.56ms)
✓ app-server ready
result: succeeded
message: app-server restarted
readiness: local ready (Remote Control degraded)
phase: steady
router: host-owned router ready
app_server: ready (0.154.0)
remote_control: disabled
remote_server_name: unavailable
remote_environment_id: unavailable
desktop attachment: configured
desktop relaunch: restart required if already running
executable_relation: matches installed executable
recovery_budget: available
last_lifecycle_outcome: succeeded

$ codex-router host router restart --port 18787 --router-root /private/tmp/host-reconnect-live.E9yJd1/router
✓ preparing router (4.62µs)
✓ stopping router (707.54µs)
✓ starting router (480.50ms)
✓ router ready
result: succeeded
message: owned router restarted
readiness: local ready (Remote Control degraded)
phase: steady
router: host-owned router ready
app_server: ready (0.154.0)
remote_control: disabled
remote_server_name: unavailable
remote_environment_id: unavailable
desktop attachment: configured
desktop relaunch: restart required if already running
executable_relation: matches installed executable
recovery_budget: available
last_lifecycle_outcome: succeeded

$ codex-router host restart --port 18787 --router-root /private/tmp/host-reconnect-live.E9yJd1/router
✓ starting Host replacement (46.41ms)
✓ stopping app-server (3.11ms)
✓ stopping router (4.13ms)
✓ re-executing Host (4.62s)
✓ router ready
✓ app-server ready
restart_result: host restarted using installed executable
readiness: local ready (Remote Control degraded)
phase: steady
router: host-owned router ready
app_server: ready (0.154.0)
remote_control: disabled
remote_server_name: Sunbook-Pro-M4.local
remote_environment_id: unassigned
desktop attachment: configured
desktop relaunch: restart required if already running
executable_relation: matches installed executable
recovery_budget: available
last_lifecycle_outcome: none
```
