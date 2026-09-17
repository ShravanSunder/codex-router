# Isolated live-proof status

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
The three live commands produced these non-TTY transcripts:

```text
codex-router host app-server restart --port 18787 --router-root <debug-router-root>
# ✓ preparing app-server (1.57s)
# ✓ stopping app-server (3.71ms)
# ✓ starting app-server (68.56ms)
# ✓ app-server ready
# result: succeeded; readiness: local ready (Remote Control degraded)

codex-router host router restart --port 18787 --router-root <debug-router-root>
# ✓ preparing router (4.62µs)
# ✓ stopping router (707.54µs)
# ✓ starting router (480.50ms)
# ✓ router ready
# result: succeeded; readiness: local ready (Remote Control degraded)

codex-router host restart --port 18787 --router-root <debug-router-root>
# ✓ starting Host replacement (46.41ms)
# ✓ stopping app-server (3.11ms)
# ✓ stopping router (4.13ms)
# ✓ re-executing Host (4.62s)
# ✓ router ready
# ✓ app-server ready
# restart_result: host restarted using installed executable
# readiness: local ready (Remote Control degraded)
```
