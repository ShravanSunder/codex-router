# codex-router

`codex-router` is a greenfield local router for Codex CLI custom-provider traffic.

The product boundary is intentionally narrow:

- Codex remains the CLI, protocol client, session owner, installer, config owner, hook runner, MCP owner, and log/session/history owner.
- `codex-router` owns only local router authentication, upstream OAuth accounts, quota snapshots, account selection, and byte-preserving forwarding of Codex model-provider traffic.
- Prodex is source-mining reference material only. This repo is not a Prodex fork.

Current design source of truth:

- [Greenfield product spec](docs/specs/2026-06-20-codex-router-greenfield-spec.md)
- [Research evidence](docs/specs/references/2026-06-20-research-evidence.md)

