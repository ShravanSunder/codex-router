# Security Reliability Lane

Status: answered
Candidate evidence label: `candidate-security-reliability-v1`
Security context: applicable

## Accepted Plan Constraints

1. Local auth is a hard ingress boundary. Accepted carriers are only
   `Authorization: Bearer` and `X-Codex-Router-Token`; mixed carriers must
   match; query, cookie, body, subprotocol, and first-frame carriers fail before
   route assessment, selector state, credentials, auth injection, or upstream
   open.
2. Affinity is fail-closed continuation correctness, not optimization. Hash
   secret load/create happens before selector advancement on response-capable
   routes, and unavailable or ambiguous owner state blocks weighted fallback.
3. Secret material stays out of SQLite. SQLite stores only full
   64-lowercase-hex HMAC owner keys and owner DTO fields.
4. WebSocket `/v1/responses` remains first-class scope. First-frame parsing is
   bounded to 1 MiB and 250 ms, reads only allowlisted top-level routing fields,
   and forwards accepted payload unchanged after selection.
5. Startup and selection use persisted SQLite selector rows. Provider quota
   refresh cannot block bind/listen, first routed request, or status rendering.
6. Redaction is shared infrastructure, not per-call formatting.
7. Observability proof is allowlisted: route band, reason enums, safe label/hash,
   call counts, and carrier enums are allowed; tokens, auth headers, secret
   paths/ids, raw bodies, full frames, prompts, and tool args are forbidden.

## Accepted Hard Gates

- Local-auth negative matrix proves zero selector, credential resolver, auth
  injection, and upstream-open calls.
- Affinity-secret unavailable proves fail-closed before selector advancement for
  HTTP/SSE and WebSocket.
- SQLite cutover proves no raw-key API/table fallback and no raw previous
  response ids are persisted.
- WebSocket first-frame matrix proves bounded parsing, auth-smuggling rejection,
  no raw frame/body emission, and zero side effects.
- Non-blocking black-box proof covers bind/listen, first request, and status
  render while refresh is delayed/failing.
- Installed-Codex e2e proves generated-profile bearer auth for HTTP/SSE and
  WebSocket with audit-safe receipts.
- Redaction proof scans status, JSON, audit, logs/traces, smoke transcripts, and
  review artifacts.

## Accepted Recovery Notes

- Hard schema cutover; old raw affinity pins are discarded or ignored.
- Missing/unreadable/replaced affinity secret fails closed and requires explicit
  owner purge/repair.
- No live OAuth/keychain expansion in this burn-down plan.

## Receipt

Answered by security-reliability lane. Parent accepted the hard gates and
recovery notes.
