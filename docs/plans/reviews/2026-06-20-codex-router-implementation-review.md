# Codex Router Implementation Review

Date: 2026-06-20
Workflow: `shravan-dev-workflow:implementation-review-swarm`
Mode: implementation
Scope: current greenfield worktree, no base commit

## Verdict

not_ready

Reason:

- Accepted blocker findings show the proxy is not yet a valid real Codex router:
  HTTP/SSE can hang or lose streaming, real HTTPS upstream HTTP routes do not
  work, WebSocket upgrades are accepted before auth/path validation, and local
  token rotation is not implemented for a running router.
- Several required proof rows are still incomplete or overclaimed: audit logging
  is a stub, quota state is not route/runtime correct, installed-Codex smoke
  does not exercise HTTP/SSE, and profile write is not truly preview-first.

## Findings

1. blocker: HTTP/SSE transport can hang, buffers streams, and cannot reach real
   HTTPS upstreams.
   Evidence:
   `crates/codex-router-proxy/src/server.rs:435` reads local requests with
   `read_to_end`; `crates/codex-router-proxy/src/upstream.rs:101` sends through
   raw `TcpStream`; `crates/codex-router-proxy/src/upstream.rs:124` rejects
   non-`http://` endpoints; `crates/codex-router-proxy/src/server.rs:488`
   rewrites responses with `Content-Length`.
   Scenario:
   A normal HTTP client that sends a request and waits without half-closing its
   write side can block the router. Even when the client half-closes, SSE is
   buffered until upstream EOF. A real `https://api.openai.com/v1` upstream is
   accepted by config but fails when HTTP routes are sent; the only working HTTP
   route path is cleartext `http://`.
   Smallest fix:
   Replace EOF-driven raw transport with framed local request parsing and an
   HTTPS-capable streaming upstream transport. Stream upstream response bytes
   without forcing `Content-Length`.
   Proof:
   Add integration proof that a normal client without `Shutdown::Write` receives
   a response, an SSE event reaches the client before upstream EOF, and
   `https://` `/v1/models` forwarding works.
   Confidence: high.
   Sources: contracts/tests, security, reliability/design.

2. blocker: WebSocket upgrades are accepted before auth/path validation and can
   block the serial serve loop.
   Evidence:
   `crates/codex-router-proxy/src/websocket.rs:251` accepts the local WebSocket
   before auth; `crates/codex-router-proxy/src/websocket.rs:262` blocks waiting
   for the first frame; `crates/codex-router-proxy/src/websocket.rs:202`
   synthesizes `/v1/responses`; `crates/codex-router-proxy/src/server.rs:281`
   always builds upstream `/v1/responses`.
   Scenario:
   A local client can complete a 101 upgrade without a router token, never send
   a first frame, and monopolize the synchronous accept loop. A valid-token
   upgrade to `/v1/realtime` or any unsupported path is treated as
   `/v1/responses` instead of failing closed before account selection.
   Smallest fix:
   Validate local auth and classify the actual handshake path before accepting
   the upgrade, carry the classified path/query into upstream URL construction,
   and enforce a bounded first-frame deadline.
   Proof:
   Add protocol/runtime tests for missing-token/no-frame WebSocket clients,
   `/v1/realtime` upgrades, and unknown upgrade paths, asserting local rejection
   with no selector or upstream connection.
   Confidence: high.
   Sources: security, contracts/tests, reliability/design.

3. blocker: local token lifecycle is incomplete for a production router.
   Evidence:
   `crates/codex-router-cli/src/lib.rs:58` loads the local token once before
   startup; `crates/codex-router-proxy/src/server.rs:196` constructs
   `LocalRouterAuth` with no previous token generations; `crates/codex-router-cli/src/token.rs:65`
   has a service method for rotation but no production CLI command to create or
   rotate the initial router token; `docs/plans/2026-06-20-codex-router-implementation-plan.md:245`
   requires runtime invalidation and WebSocket close behavior.
   Scenario:
   A fresh router root cannot create/export an initial token through a supported
   production command. If token material changes while `serve` is running, the
   running router keeps accepting the startup token and rejects the new token
   until restart. Existing WebSockets cannot be closed by generation.
   Smallest fix:
   Add explicit init/rotate CLI surfaces, persist current and previous
   generation metadata, make runtime auth reloadable, and track active
   WebSocket sessions by token generation for redacted close-on-rotation.
   Proof:
   Start `serve`, create/export token A, rotate to token B without restart,
   prove new HTTP/WS handshakes require B, old-token WebSockets close with a
   redacted reason, and committed HTTP/SSE responses can finish.
   Confidence: high.
   Sources: spec/proof, reliability/design.

4. important: audit logging is a schema stub, not the required private
   router-root sink.
   Evidence:
   `docs/plans/2026-06-20-codex-router-implementation-plan.md:240` requires a
   private sink and proxy audit proof; `crates/codex-router-core/src/audit.rs`
   defines only a small event shape; repository search finds no proxy writer or
   emitter path.
   Scenario:
   Local auth rejection and routing decisions produce no audit file under the
   router root. The current event shape cannot represent required allowlisted
   fields such as transport kind, account hash, quota bands, reservation id,
   affinity hash, precommit rotations, response commit state, or error class.
   Smallest fix:
   Implement a private router-root audit sink, expand the allowlisted schema,
   and emit redacted audit records from HTTP/SSE and WebSocket flows.
   Proof:
   Integration test for auth rejection and successful forwarding that asserts
   private audit file creation, required fields, and absence of token/body/email
   canaries.
   Confidence: high.
   Sources: spec/proof.

5. important: quota state and selection runtime do not preserve route-specific
   or process-lifetime balancing.
   Evidence:
   `crates/codex-router-state/src/sqlite.rs:326` keys `quota_snapshots` only by
   `account_id`; `crates/codex-router-state/src/sqlite.rs:187` overwrites the
   route band on conflict; `crates/codex-router-cli/src/lib.rs:250` defaults
   `now_unix_seconds` to `0`; `crates/codex-router-proxy/src/server.rs:273` and
   `:295` recreate repository-backed selectors per connection.
   Scenario:
   Persisting quota for multiple supported routes on one account overwrites the
   previous route band. Real timestamped snapshots are classified as unknown by
   default serve config, and weighted deficit state resets per connection, so
   repeated one-request connections can bias toward the first eligible account
   instead of balancing.
   Smallest fix:
   Persist quota by `(account_id, route_band)` or store all route bands per
   account, default the runtime clock from system time, and keep selector /
   reservation state alive for the process lifetime.
   Proof:
   Repository-backed selector tests for multiple route bands on one account and
   CLI/runtime tests proving fresh snapshots and cross-connection account
   distribution.
   Confidence: high.
   Sources: contracts/tests, reliability/design.

6. important: installed-Codex smoke does not prove the HTTP/SSE side of R20.
   Evidence:
   `docs/plans/2026-06-20-codex-router-implementation-plan.md:261` says the
   smoke exercises HTTP/SSE and WebSocket paths; the latest transcript
   `tmp/smoke/installed-codex-mock-26264-1781989282085.json` records
   `http_probe_count: 0` and one WebSocket handshake.
   Scenario:
   The installed-Codex smoke can pass while HTTP/SSE custom-provider traffic is
   broken, because it validates only the WebSocket path.
   Smallest fix:
   Add installed-Codex or equivalent real-Codex custom-provider smoke coverage
   for HTTP/SSE, or revise R20 through plan review if current Codex cannot
   exercise that path.
   Proof:
   Redacted transcript with verified HTTP/SSE and WebSocket exchanges, including
   auth stripping and upstream auth injection on both.
   Confidence: high.
   Sources: spec/proof.

7. important: profile write is not a true preview-first home mutation workflow.
   Evidence:
   `docs/specs/2026-06-20-codex-router-greenfield-spec.md:43` requires a
   preview-first workflow; `crates/codex-router-cli/src/profile.rs:85` writes as
   soon as `approved` is true; `crates/codex-router-cli/src/lib.rs:115` dry-run
   prints target plus replacement content but no existing-file diff or preview
   acknowledgement.
   Scenario:
   A user or script can mutate a real `~/.codex` profile in one command without
   an enforced preview receipt. Existing profile overwrites are not shown as a
   deterministic diff.
   Smallest fix:
   Enforce a two-step preview/confirm flow tied to target/content or add a
   deterministic dry-run diff plus explicit confirmation token for writes.
   Proof:
   CLI tests showing write fails without preview acknowledgement, dry-run
   displays old/new deltas when the target exists, and temp `CODEX_HOME`
   preview-confirm writes only the named profile file.
   Confidence: high.
   Sources: security, spec/proof.

## Open Questions

- Whether HTTP/SSE should be fixed with an embedded Rust HTTP client or a lower
  level transport abstraction is an implementation design choice. The current
  raw EOF/TcpStream path is not sufficient either way.
- Whether profile write confirmation should use a content hash, target hash, or
  explicit generated confirmation token can be decided during the route-back
  implementation pass.

## Review Proof

- Implementation proof was checked against the reviewed spec, reviewed plan,
  implementation provenance, smoke transcript, and current code.
- Required proof was found missing or overclaimed for R3, R7, R8/R20, R11B,
  R14/R15, and R19.
- Red/green evidence exists for many local tests, but accepted findings show
  those tests do not cover key production behaviors.
- No explicit user-approved exception waives these missing proof rows.

## Swarm Coverage

- Spec compliance and implementation proof lane: ran via Codex reviewer
  subagent, 4 raw findings.
- Security and trust-boundary lane: ran via Codex reviewer subagent, 3 raw
  findings.
- Contracts and tests lane: ran via Codex reviewer subagent, 4 raw findings.
- Reliability, performance, and adversarial design lane: ran via Codex reviewer
  subagent, 4 raw findings.
- External counsel: not run; user did not request Claude, Gemini, `agy`, or
  another outside model lane.
- Reducer result: 15 raw findings were deduplicated into 7 accepted findings.

## Routing Follow-Through

- Accepted blocker and important findings route back to
  `shravan-dev-workflow:implementation-execute-plan`.
- No same-session tiny review fix was attempted because the accepted findings
  require substantial design and proof changes across transport, auth lifecycle,
  audit, quota, smoke, and profile workflows.
- PR readiness remains blocked by accepted implementation findings.

## Artifact Links

- Implementation review packet:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/plan-workflows/2026-06-20-codex-router-main-implementation-review/implementation-review-packet.md`
- Implementation provenance:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/docs/wip/implementation-provenance.md`
- Latest smoke transcript:
  `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/tmp/smoke/installed-codex-mock-26264-1781989282085.json`

phase_result: needs_revision
evidence: this review report, implementation review packet, reviewer lane outputs, code citations above, `git diff --check`
recommended_next_workflow: shravan-dev-workflow:implementation-execute-plan
recommended_transition_reason: Accepted blocker findings make the implementation not ready; fixes and proof belong to implementation execution before another review.
