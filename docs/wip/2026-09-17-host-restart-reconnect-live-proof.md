# Isolated live-proof status

The fixture-backed compiled CLI acceptance is green, including whole-Host
replacement from a new install path. A direct isolated debug Host launch was
attempted with fresh `/private/tmp` router, Codex, and socket roots and port
`18787`.

The sandboxed attempt was rejected before startup because local network access
to `127.0.0.1` is unavailable. The escalated attempt reached the binary but
exited before publishing the operator socket with:

```text
debug Codex profile could not be read
```

No production process, port `8787`, `~/.codex-router`, or `~/.codex` state was
modified. Once the owner-managed debug profile is readable, rerun:

```text
codex-router host app-server restart --port 18787 --router-root <debug-router-root>
codex-router host router restart --port 18787 --router-root <debug-router-root>
codex-router host restart --port 18787 --router-root <debug-router-root>
```
