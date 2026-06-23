# R18 Requirements And Validation Lane

Verdict: needs revision

Accepted by parent:

- Blocker: black-box fail-closed proof named unsupported paths but not wrong
  HTTP methods on otherwise supported paths.

What held:

- Supported-route route-native proof covered `POST /v1/responses`, WebSocket
  `/v1/responses`, `GET /v1/models`, `POST /v1/memories/trace_summarize`, and
  `POST /v1/responses/compact`.
- Selector proof covered route-band isolation, cooldown debit, affinity
  no-debit, reserve-owner continuity, and fail-closed owner states.

Receipt:

- Source anchors: spec route inventory/proof rows, `routes.rs`, installed-Codex
  harness, proxy tests.
- Parent reducer wrote this lane summary from the subagent candidate output.
