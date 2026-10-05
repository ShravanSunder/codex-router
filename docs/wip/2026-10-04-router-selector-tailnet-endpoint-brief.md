# Router selector tailnet-only endpoint proposal

Status: research/design proposal for owner approval. No Tailscale, SSH, Router, auth, credential, or configuration change was made.

## Read-only observations

### Tailscale on Sunclaw

- Read-only launchd evidence reports `system/homebrew.mxcl.tailscale` running (PID 652, no prior exit) and `/var/run/tailscaled.socket` present. The current CLI and launchd daemon are both Homebrew Tailscale `1.102.5` in that evidence.
- In the managed shell, direct AF_UNIX access to the socket returns `PermissionError: [Errno 1] Operation not permitted`; `ps` is also denied. Both `tailscale status` and explicit `--socket=/var/run/tailscaled.socket status` then report failure to connect/enumerate.
- This is an environment IPC/process-enumeration gap. It does not establish that Tailscaled is down, and no VPN restart or repair was attempted. An earlier shell observation succeeded, so status results are shell-context dependent.
- `tailscale serve status` reports **No serve config** in the successful read-only CLI context.

### Router endpoint contracts

- Collaboration MCP is an HTTP streamable listener bound only to loopback. The normal host default is `127.0.0.1:8788`; isolated debug uses `127.0.0.1:18788`. The listener serves `/mcp`, validates loopback bind addresses, and allows local host/origin values: `crates/collaboration-mcp/src/mcp_http_listener.rs:25-60`; `crates/codex-router-cli/src/host_command/mod.rs:353-368`.
- The MCP listener's remote credential presentation/auth contract is not established by this source. Tailscale transport would not by itself establish Router service identity or permitted MCP tools.
- The collaboration control socket and native Codex channel are owner-local Unix endpoints. `ControlClient::connect` requires an absolute service directory, reads `service.json`, resolves `control.sock`, and validates service ID, epoch and control schema: `crates/collaboration-client/src/service_discovery.rs:12-56`.
- Endpoint inventory advertises native Codex as `unixWebSocket` with a relative path such as `codex-native.sock`; the current launcher projects `--remote unix://<socket-path>`: `crates/collaboration-client/src/native_endpoint_selector.rs:9-75`; `crates/codex-native-integration/src/native_session_launch.rs:305-317`.
- The current Router source has no network native `ws://`/`wss://` exposure or attachment-time service binding. The selector design already records that upstream remote support and Router binding are separate unverified prerequisites: `docs/specs/2026-10-03-router-selector/2026-10-03-router-selector-program-design.md:116-149`.

## Serve versus SSH forwarding

### Tailscale Serve

Serve is tailnet-only by default and can proxy a local port or Unix socket. The CLI help confirms `tailscale serve <target>`, including `tailscale serve 8788` and `tailscale serve unix:<socket>`. It is therefore technically capable of carrying the loopback MCP HTTP surface and, in a separate experiment, could proxy a Unix socket.

The current contract still leaves three gaps: persistent Serve configuration is an owner-authorized exposure change; MCP host/origin and application-auth behavior through Serve are unverified; and proxying a Unix WebSocket does not establish the Router service/endpoint identity and native attachment binding required for remote Codex NEW/fork. Serve is not selected for this slice. Funnel is explicitly excluded.

### SSH forwarding

An operator-supplied SSH local forward can carry the existing loopback MCP surface without persistent Tailscale Serve state, for example a local ephemeral port forwarding to Sunbook's `127.0.0.1:8788`. Tailscale SSH's wrapper resolves MagicDNS and supplies a ProxyCommand, but it does not add Router service identity, MCP auth, or native session binding. SSH server availability, authorization policy, host-key binding and the remote MCP credential contract are not verified here.

Plain SSH local forwarding does not carry the current native Unix WebSocket directly. Native NEW/fork would still require a remote TCP/WebSocket app-server exposure or an additional proxy, which is outside this authorized research and would need its own identity/auth contract.

## Recommendation

For a future owner-approved, read-only machine-inventory path, prefer **operator-supplied SSH forwarding to the loopback MCP listener** over Tailscale Serve. It avoids a persistent Serve configuration and keeps the path tailnet-only, while making the SSH/auth/binding responsibility explicit. The registry would reference an already-qualified MCP URL/credential mechanism; it would not configure SSH, read credentials, or infer service identity.

Do **not** enable remote native NEW or fork through either path yet. Keep those actions rejected until an existing Router-owned `ws://`/`wss://` exposure proves service/endpoint identity, attachment-time binding, credential presentation and permitted policy projection. Do not add a relay, proxy, Funnel exposure, wildcard bind, or all-endpoint publication to close the gap.

## Owner approval question

After the existing F2 source-view decision is answered, approve SSH forwarding as an operator-supplied MCP-only discovery path, with native remote NEW/fork remaining disabled until a bound Router-native `ws://`/`wss://` contract exists?

This proposal does not change the JSONC schema, run SSH, configure Serve, alter Tailscale, or authorize credentials. The fixes Lead's UUID restore work remains separate.
