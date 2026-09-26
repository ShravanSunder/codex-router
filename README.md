# codex-router

`codex-router` is a greenfield local router for Codex CLI custom-provider traffic.

The product boundary is intentionally narrow:

- Codex remains the CLI, protocol client, session owner, installer, config owner, hook runner, MCP owner, and log/session/history owner.
- `codex-router serve` owns local router authentication, upstream OAuth accounts, quota snapshots, account selection, and byte-preserving forwarding of Codex model-provider traffic.
- The optional foreground `codex-router host` command owns one local router and one native Codex app-server child, and composes owner-local Control, native Codex relay, and ACP channels. Codex still owns native threads, queues, permissions, and agent execution.
- The separate `agent-collaboration` CLI and reusable `collaboration-client` Rust SDK provide session discovery, explicit messaging, observation, and exact interruption. Lifecycle metadata lives separately from provider state and Codex history.
- Prodex is source-mining reference material only. This repo is not a Prodex fork.

The collaboration CLI is an interface to the Rust SDK. Reusable connection and
session operations belong to `collaboration-client`; `collaboration-protocol`
defines the shared RPC contracts. `collaboration-service` handles requests inside
the Router Host. Message-board types and rules live in `message-board`, with SQLx
persistence in `message-board-storage`.
RPC schemas and types are available through `collaboration_client::protocol`;
board request/result types through `collaboration_client::board`. SDK consumers
do not need to invoke the CLI or depend on its argument types.

Current design source of truth:

- [Greenfield product spec](docs/specs/2026-06-20-codex-router-greenfield-spec.md)
- [Research evidence](docs/specs/references/2026-06-20-research-evidence.md)
- [Agent communication requirements](docs/specs/2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-requirements.md), [Specification](docs/specs/2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-specification.md), and [Program Design](docs/specs/2026-09-05-agent-communication-system/2026-09-05-agent-communication-system-program-design.md)

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
the managed Codex app-server and enabled ACP providers, and keeps lifecycle
control on an owner-only Unix socket. On first start it creates owner-editable
`<router-root>/providers.json` with Claude and Cursor enabled. The default
executables are `claude-agent-acp` and `agent acp`; explicit Host provider flags
override the file for that start. If one provider is unavailable, the endpoint
catalog reports its reason and fix while the other endpoints remain available.

Hosted `agent-sessions` new/resume launches
resolve the advertised public native selector. Backend replacement closes native
connections; the native TUI owns bounded reconnection without a Sessions supervisor.

```shell
cargo run -p agent-collaboration --bin agent-sessions -- --id 019fe7c6-f493-7f02-be72-2feac69d6e6d
cargo run -p codex-router-cli -- host status
cargo run -p codex-router-cli -- host restart
cargo run -p codex-router-cli -- host app-server restart
cargo run -p codex-router-cli -- host app-server update
cargo run -p codex-router-cli -- host router restart
```

`host restart` replaces the whole Host with the installed CLI issuing the
command and waits for replacement readiness. It retains the Host's configuration
and does not download a binary. For an installed Host, run `codex-router host
restart` after installing the newer version.

`host app-server restart` restarts only managed Codex without updating it.
`host app-server update` runs the managed Codex updater. If executable content
changes, the foreground Host stops its children and re-execs itself; otherwise
the running app-server and connected clients are left untouched. `host update`
is no longer a command. This is a foreground runtime, not a background service
or launchd agent.

**First upgrade from a Host without whole-Host restart support:** stop the old
foreground Host in its owning terminal, wait for it to exit, then start the
newly installed `codex-router host` once. The old process cannot understand the
new restart request. Subsequent compatible upgrades use `host restart`.
See [Host restart validation](docs/testing/host-restart.md) for isolation and
replacement proof.

For discovery, agent-declared messages, queue/steer, timed wake-ups and scheduled
work, see the [agent CLI guide](docs/agent-guidance/agent-collaboration.md).
`agent-collaboration conversation create|prompt|load|cancel` uses the target
endpoint's client, including Codex, Claude, and Cursor. Use `conversation
operation show|wait|reconcile` for inspectable operations. `message send` can
also reach a live Claude Code session through its peer socket; its
`peerMessageWritten` receipt confirms the write, not the session's response.
That guide also documents registering the running Router Host's manifest-advertised
Streamable HTTP MCP endpoint with Codex. Use the selected current `service.json`;
do not guess a port or expose the unauthenticated endpoint beyond loopback.
Automation uses a separate `automation.sqlite` database and the same Rust SDK
and CLI. [Debug testing instructions](docs/testing/automation-debug-testing.md)
cover the opt-in Luna acceptance runner and its isolated Host. Shared message boards use the same client and service; see the
[agent collaboration skill](agent-skills/agent-collaboration/SKILL.md). Other language
SDK implementations and remote transport follow separately.

## Install and upgrade

Install `codex-router` from the Homebrew tap:

```shell
brew tap shravansunder/taps
brew trust shravansunder/taps  # Homebrew 7+
brew install shravansunder/taps/codex-router
```

Upgrade the installed binaries and restart a running Host:

```shell
brew update && brew upgrade codex-router
codex-router host restart
```

A running Host continues using the build it started with until restarted. `codex-router host status` and the `agent-collaboration` CLI warn when the installed or connected Router version is newer.

If Homebrew reports conflicts in its tap clone, inspect and restore only the formula file, then fast-forward the tap:

```shell
git -C "$(brew --repository)/Library/Taps/shravansunder/homebrew-taps" status
git -C "$(brew --repository)/Library/Taps/shravansunder/homebrew-taps" checkout -- Formula/codex-router.rb
git -C "$(brew --repository)/Library/Taps/shravansunder/homebrew-taps" pull --ff-only
```

Do not edit the installed tap clone; formula changes go through the release workflow.
