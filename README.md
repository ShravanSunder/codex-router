# codex-router

`codex-router` is a greenfield local router for Codex CLI custom-provider traffic.

The product boundary is intentionally narrow:

- Codex remains the CLI, protocol client, session owner, installer, config owner, hook runner, MCP owner, and log/session/history owner.
- `codex-router` owns only local router authentication, upstream OAuth accounts, quota snapshots, account selection, and byte-preserving forwarding of Codex model-provider traffic.
- Prodex is source-mining reference material only. This repo is not a Prodex fork.

Current design source of truth:

- [Greenfield product spec](docs/specs/2026-06-20-codex-router-greenfield-spec.md)
- [Research evidence](docs/specs/references/2026-06-20-research-evidence.md)

## Current Local Flow

Use a router-owned root for account state and upstream OAuth credentials:

```shell
ROUTER_ROOT=/path/to/codex-router-state
cargo run -p codex-router-cli -- account login --router-root "$ROUTER_ROOT" --label primary --device-auth --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account login --router-root "$ROUTER_ROOT" --label backup --auth-json /path/to/auth.json --allow-plaintext-file-secrets
cargo run -p codex-router-cli -- account list --router-root "$ROUTER_ROOT"
cargo run -p codex-router-cli -- quota refresh --router-root "$ROUTER_ROOT"
cargo run -p codex-router-cli -- quota status --router-root "$ROUTER_ROOT" --all-limits
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
cargo run -p codex-router-cli -- token init --router-root "$ROUTER_ROOT/secrets"
cargo run -p codex-router-cli -- token export --router-root "$ROUTER_ROOT/secrets" --shell posix
cargo run -p codex-router-cli -- serve \
  --state-db "$ROUTER_ROOT/state.sqlite" \
  --secret-root "$ROUTER_ROOT/secrets" \
  --upstream-base-url https://chatgpt.com/backend-api \
  --quota-refresh-interval-seconds 300
```

Startup does not block on quota refresh. `serve` reads last-known SQLite quota
state immediately, starts an immediate background refresh after binding, and
continues refreshing on the configured schedule. Run `quota refresh` for an
explicit manual provider fetch, and `quota status` for SQLite-only status
output.
