# codex-router

`codex-router` is a greenfield local router for Codex CLI custom-provider traffic.

The product boundary is intentionally narrow:

- Codex remains the CLI, protocol client, session owner, installer, config owner, hook runner, MCP owner, and log/session/history owner.
- `codex-router serve` owns local router authentication, upstream OAuth accounts, quota snapshots, account selection, and byte-preserving forwarding of Codex model-provider traffic.
- The optional foreground `codex-router host` command owns one local router and one native Codex app-server child. Codex clients connect directly to the app-server Unix socket; the host never proxies Codex protocol traffic.
- Prodex is source-mining reference material only. This repo is not a Prodex fork.

Current design source of truth:

- [Greenfield product spec](docs/specs/2026-06-20-codex-router-greenfield-spec.md)
- [Research evidence](docs/specs/references/2026-06-20-research-evidence.md)

## Current Local Flow

By default, installed or release `codex-router` stores router-owned state under
`$HOME/.codex-router`, for example `/Users/shravansunder/.codex-router` on this
machine. Debug `cargo run -p codex-router-cli -- ...` builds default to
`$HOME/.codex-router-debug` so local development does not touch the production
router root. Use `--router-root <path>` only for tests or an alternate local
router home.

```shell
cargo run -p codex-router-cli -- account login --label primary --device-auth --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account login --label backup --auth-json /path/to/auth.json --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account list
cargo run -p codex-router-cli -- quota refresh
cargo run -p codex-router-cli -- quota status --all-limits
```

`account login --device-auth` delegates the browser/device-code OAuth step to
the installed `codex` binary in a temporary owner-only `CODEX_HOME`, then imports
the resulting OAuth `auth.json` into router-owned account state. Use
`--codex-bin <path>` to point at a specific Codex binary.

`account login --auth-json` is the explicit import path for an existing
Codex/Prodex-style OAuth `auth.json`. It is useful for migration, recovery, and
test setup. API-key auth is not quota-compatible.

Start the local router from the same persisted state:

```shell
cargo run -p codex-router-cli -- serve \
  --quota-refresh-interval-seconds 300
```

Startup does not require `CODEX_ROUTER_TOKEN` and does not block on quota
refresh. `serve` reads last-known SQLite quota state immediately, starts an
immediate background refresh after binding, and continues refreshing on the
configured schedule. Run `quota refresh` for an explicit manual provider fetch,
and `quota status` for SQLite-only status output.

## Shared Codex Host

Start the personal-use shared host manually in a foreground terminal:

```shell
cargo run -p codex-router-cli -- host
```

The host starts `codex-router serve` when a compatible router is absent, starts
the managed Codex app-server with Remote Control enabled, and keeps lifecycle
control on an owner-only Unix socket. Existing `sessions` new/resume launches
attach directly to Codex's conventional app-server socket.

```shell
cargo run -p codex-router-cli -- host status
cargo run -p codex-router-cli -- host restart
cargo run -p codex-router-cli -- host restart-router
cargo run -p codex-router-cli -- host update
```

`host update` runs the managed Codex updater. If executable content changes,
the foreground host stops its children and re-execs itself; otherwise the
running app-server and connected clients are left untouched. This MVP is not a
background service, launchd agent, client proxy, or cross-machine control
plane.
