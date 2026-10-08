# codex-router

`codex-router` is a greenfield local router for Codex CLI custom-provider traffic.

The product boundary is intentionally narrow:

- Codex remains the CLI, protocol client, session owner, installer, config owner, hook runner, MCP owner, and log/session/history owner.
- `codex-router serve` owns local router authentication, upstream OAuth accounts, quota snapshots, account selection, and byte-preserving forwarding of Codex model-provider traffic.
- The optional foreground `codex-router host` command owns one local router and one native Codex app-server child, serves the collaboration API as MCP tools, and composes the native Codex relay and ACP channels. Codex still owns native threads, queues, permissions, and agent execution.
- The separate `agent-collaboration` CLI and reusable `collaboration-client` Rust SDK provide session discovery, explicit messaging, observation, and exact interruption. Lifecycle metadata lives separately from provider state and Codex history.
- Prodex is source-mining reference material only. This repo is not a Prodex fork.

The collaboration CLI is an interface to the Rust SDK. Reusable connection and
session operations belong to `collaboration-client`; `collaboration-protocol`
defines the shared request and result types. `collaboration-service` handles
requests inside the Router Host, and `collaboration-mcp` serves them as MCP tools. Message-board types and rules live in `message-board`, with SQLx
persistence in `message-board-storage`.
Request and result types are available through `collaboration_client::protocol`;
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
cargo run -p codex-router-cli -- account list
cargo run -p codex-router-cli -- quota refresh
cargo run -p codex-router-cli -- quota status --all-limits
```

`account login --device-auth` delegates the browser/device-code OAuth step to
the installed `codex` binary in a temporary owner-only `CODEX_HOME`, then imports
the resulting OAuth `auth.json` into router-owned account state. Use
`--codex-bin <path>` to point at a specific Codex binary.

The Router keeps enabled accounts' renewable credentials current while it runs,
including when an account is idle or its quota is exhausted. A disabled account
is not maintained until it is re-enabled. Re-login through device auth when
account or quota status reports that renewal needs user action.

Start the local router from the same persisted state:

```shell
cargo run -p codex-router-cli -- serve
```

Startup does not require `CODEX_ROUTER_TOKEN` and does not block on quota
refresh. `serve` reads last-known SQLite quota state immediately, starts an
immediate background refresh after binding, then targets a new quota observation
every 180 seconds by default. OAuth upkeep runs separately even when background
quota refresh is disabled. Run `quota refresh` for an explicit manual provider fetch,
and `quota status` for SQLite-only status output.

In an interactive terminal, `quota status` lets you browse accounts. Press
`Ctrl-R` to open options for the focused account, then use Tab or the arrow keys
to switch between Resets and Credits. On Credits, press `r` on an eligible
account to refresh provider quota and credit observations for the whole pool.
Press Enter to edit the saved usage policy
(`Disallow` by default), use the arrow keys to choose, Enter to save, or Esc to
cancel. Credits are the last resort: every eligible account's included quota
comes first. `Allow` permits credit use for Responses, compact, and image
generation/edit only with current provider-confirmed credits, matching
credentials, and no floor or provider rejection. Existing credit-backed
sessions yield when included quota becomes available. This preference does not
set a spending limit; the provider controls the actual debit.

## Shared Codex Host

Start the personal-use shared host manually in a foreground terminal:

```shell
cargo run -p codex-router-cli -- host
```

The host starts `codex-router serve` when a compatible router is absent, starts
the managed Codex app-server and enabled ACP providers, and answers its lifecycle
commands (`host status`, `host restart`) on a separate owner-only operator socket.
On first start it creates owner-editable
`<router-root>/providers.json` with Claude and Cursor enabled. The default
executables are `claude-agent-acp` and `agent acp`; explicit Host provider flags
override the file for that start. If one provider is unavailable, the endpoint
catalog reports its reason and fix while the other endpoints remain available.

The Host serves one set of collaboration tools over MCP (Streamable HTTP) on two
listeners: the owner-only Unix socket `control.sock` in its service directory,
which the CLIs and SDK use, and a loopback TCP URL for models. It publishes
`service.json` (manifest version 3) beside the socket: `serviceId`,
`serviceEpoch`, `machineLabel`, `serviceVersion`, `api` (the socket), `mcp.url`
(the TCP URL), `nativeSchemaDigest`, and `routerProxyEndpoint` when the proxy
runs. Clients read these values from the manifest; there is no connection
handshake, and older manifest versions are refused, so install the CLI and Host
together.

Waits and observations are single bounded calls that return the cursor to resume
from. `events_observe` also streams each event while its call is open, as a
`notifications/codexRouter/observationEvent` notification carrying `{event,
cursor}` (the call is then answered as SSE); its result still lists every event,
so a client that ignores the notification loses nothing. `agent-collaboration
events observe --stream` prints each streamed event as an `observationEvent`
line before the result.

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
