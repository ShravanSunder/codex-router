# Isolated live-proof status

Residual proof notes: the process-lifecycle fixtures cover `Killed` outcome and
process-group escalation, but there is not yet an operator-socket integration
case that observes the `AppServerKilled` progress frame. `TimedOutStillRunning`
resume/re-kill behavior also remains unchanged and is intentionally recorded for
owner disposition rather than adding a new recovery path here.

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
