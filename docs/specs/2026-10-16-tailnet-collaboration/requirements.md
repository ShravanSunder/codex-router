# Private collaboration MCP on the owner’s tailnet

The owner works between Sunclaw and Sunbook and wants their collaboration boards and MCP tools reachable from either machine, without manually exposing each service. On 2026-10-16 the owner requested an automatic `codex-router host --xxxx` option, confirmed private Tailscale MCP/boards exposure on both machines, and required actual debug Router testing on both sides before merge. Existing Source quota/Selector delivery remains required.

- U1: A Host can opt into private Tailscale exposure with one launch option and report the connection address.
- U2: A client on either authorized tailnet machine can use the other Host’s real collaboration MCP and read/write that Host’s boards.
- U3: Exposure follows that Host’s lifetime, preserves unrelated Serve configurations, and reports setup/failure rather than falsely claiming remote readiness.
- U4: Preserve each Host’s identity, boards and local clients. Do not replicate databases or expose the operator socket, raw Codex backend, provider proxy, credentials or public Internet access.
- U5: Default launches retain the current local-only behavior. Validate the feature with actual debug Hosts on both machines, then perform affected review, CI and release checks before merge.

The owner explicitly selected an unprivileged loopback HTTP backend on 2026-10-16, preserving Origin checks. Each node’s private HTTPS `/mcp` forwards to its own existing loopback MCP listener; the Host does not need administrator privileges.

The owner’s tailnet access policy is the authorization boundary for remote collaboration MCP tools. Enabling this option grants tailnet peers permitted by that policy the existing MCP tool surface, not merely a static board listing. It does not turn self-reported board actor fields into authentication. No Tailscale policy is changed automatically.

This is an opt-in server-exposure feature, not mesh database federation, universal service discovery, remote Codex-native transport, a provider installation, or authority to replace production processes. The separately reviewed Selector SSH route retains its own native-action proof obligations. No graphical UI is introduced; the user-facing surface is a CLI option, readiness output and errors.
