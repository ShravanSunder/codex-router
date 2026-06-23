# Focused Plan Review Lane: Execution Order And Scope Closure

Verdict: `needs revision`

## Accepted Findings

### Blocker: T9/T10 parallelism was still unsafe

- Problem: T9 and T10 both owned the installed-Codex harness and smoke script,
  while the plan still allowed them to run in parallel.
- Failure: transport proof lanes could collide on `installed_codex.rs`, the
  smoke script, or transcript/receipt artifacts.
- Plan edit: T9 and T10 are now serial. T8a freezes exact smoke commands and
  transport-specific evidence roots; T9 owns HTTP/SSE first, then T10 owns
  WebSocket.

### Blocker: T7 still overlapped T6 on `server.rs`

- Problem: T7 was allowed beside T6 even though both could edit
  `crates/codex-router-proxy/src/server.rs`.
- Failure: non-blocking proof changes could collide with WebSocket pre-upgrade
  flow changes and invalidate call-order counters.
- Plan edit: T7 now starts after T6 for product-code changes. Earlier T7 work is
  allowed only when tests-only and not editing `server.rs`,
  `account_selection.rs`, or `quota.rs`.

### Important: T5 blurred primitive proof with WebSocket ingress proof

- Problem: T5 did not own `websocket.rs` or `server.rs`, but its proof text
  claimed WebSocket subprotocol and call-counter proof.
- Failure: implementers could either overreach T5 or split WebSocket auth
  inconsistently.
- Plan edit: T5 now owns shared local-auth primitive tests only for WebSocket
  carrier inputs. Actual WebSocket ingress, non-101, subprotocol, and
  call-counter proof belongs to T6.

## Closure Status

- T2 split: closed.
- T3 -> T5 -> T6 serialization: closed.
- T8a/T8b split: mostly closed, with T8a now freezing smoke commands and
  evidence roots.
- T9/T10 transport proof: closed by serialization and disjoint evidence roots.
- T7/T11 allowlists: T7 overlap closed by serialization rule.

