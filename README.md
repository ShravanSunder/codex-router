# codex-router

`codex-router` is a greenfield local router for Codex CLI custom-provider traffic.

The product boundary is intentionally narrow:

- Codex remains the CLI, protocol client, session owner, installer, config owner, hook runner, MCP owner, and log/session/history owner.
- `codex-router` owns only local router authentication, upstream OAuth accounts, quota snapshots, account selection, and byte-preserving forwarding of Codex model-provider traffic.
- Prodex is source-mining reference material only. This repo is not a Prodex fork.

Current design source of truth:

- [Greenfield product spec](docs/specs/2026-06-20-codex-router-greenfield-spec.md)
- [Research evidence](docs/specs/references/2026-06-20-research-evidence.md)

## Current Local Setup Flow

The current router-owned OAuth onboarding path is explicit import from an
existing Codex/Prodex-style OAuth `auth.json`. There is not yet an interactive
`codex-router account login` browser/device flow.

By default, `codex-router` stores router-owned state under
`$HOME/.codex-router`, for example `/Users/shravansunder/.codex-router` on this
machine. Use `--router-root <path>` only for tests or an alternate local router
home.

```shell
cargo run -p codex-router-cli -- token init

cargo run -p codex-router-cli -- account import-codex-auth \
  --label <local-safe-label> \
  --auth-json <path-to-codex-auth.json> \
  --allow-plaintext-file-secrets

cargo run -p codex-router-cli -- account list
cargo run -p codex-router-cli -- quota refresh
cargo run -p codex-router-cli -- quota status
cargo run -p codex-router-cli -- quota status --format plain
cargo run -p codex-router-cli -- quota status --all-limits
```

`$HOME/.codex-router/state.sqlite` stores account metadata and quota snapshots.
`$HOME/.codex-router/secrets` stores the local router bearer token and imported
upstream OAuth token material through the current file secret backend. That
backend is plaintext at rest under private filesystem permissions, so importing
OAuth material requires `--allow-plaintext-file-secrets`. The router root must
not be inside `.codex` or `.prodex`.

Account labels are local display hints, not account identity. Imports reject
labels outside the local-safe alphabet `A-Z`, `a-z`, `0-9`, `.`, `_`, and `-`,
and reject duplicate labels instead of replacing existing credentials.

`quota refresh` and the serve background quota worker call the provider quota
endpoint and write normalized quota state to SQLite. By default, quota fetches
only allow the provider endpoint at `https://chatgpt.com/backend-api`.
Loopback/mock quota URLs require the explicit test-only
`--allow-insecure-quota-base-url` flag. `quota status` performs no provider I/O;
it reads SQLite only. The default table shows compact effective rows, `--format
plain` emits one machine-readable line per row with percent-encoded values, and
`--all-limits` expands provider windows. Provider response quota is persisted for
the route bands the proxy selects on: `responses`, `models`,
`memories_trace_summarize`, and `responses_compact`.

Run the router with the same root:

```shell
cargo run -p codex-router-cli -- serve \
  --quota-refresh-interval-seconds 300 \
  --quota-refresh-timeout-seconds 30
```

While serving, request-time routing reads existing SQLite snapshots. Broad
provider quota refresh runs in the background and periodically writes updated
quota state back to SQLite. Invalid quota-refresh endpoint configuration fails
before listening; later background refresh failures emit a redacted stderr line.
