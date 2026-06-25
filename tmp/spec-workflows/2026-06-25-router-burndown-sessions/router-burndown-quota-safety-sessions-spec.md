# Spec: Router Burndown, Codex-Safe Quota, Sessions

Date: 2026-06-25
Status: reviewed once; accepted spec-review-cycle-1 findings applied

## Product Model

`codex-router` is an account router. It chooses an authenticated Codex account, routes Codex traffic through that account, keeps quota state fresh enough to avoid exhausted accounts, and otherwise preserves Codex HTTP/WebSocket behavior as pass-through.

The router must not invent protocol semantics beyond account routing, auth, quota safety, and connection-boundary account selection.

## Scope

This spec covers:

1. Historical quota burndown and account selection.
2. Codex-safe quota exhaustion and connection retirement.
3. Router-owned session picker/list/last command.
4. E2E proof for quota/reset/reset-credit/rate-limit behavior.

This spec supersedes the prior reset-aware burn-down non-goals that excluded forecasting, historical quota history, and active-load projection.

## Requirement 1: Historical Quota Burndown

### State Access Boundary

All new or extended SQL access for this implementation must use SQLx. Do not add or extend rusqlite queries, repository traits, migrations, session-state readers, or test helpers for quota history, quota status, account selection, or sessions. Existing rusqlite code may remain only where untouched legacy paths still require it.

### Data Contract

The state layer must persist append-only quota observations for at least the last seven days.

Required dimensions:
- account id
- safe account label
- route band
- limit window seconds
- observed timestamp
- remaining headroom percent
- reset timestamp
- window status
- effective flag
- refresh source
- refresh success/failure status
- reset credits available when provider reports it

Current latest-row tables may remain as materialized current state, but they are not enough for the new algorithm.

### Estimator Contract

The selector must compute an observed run-rate per account, route band, and window from recent history.

Minimum estimator behavior:
- no history: unknown-run-rate, usable only by explicit fallback policy
- one sample: insufficient-rate, use current quota but no slope confidence
- two samples in the same reset segment: low-confidence slope
- three or more samples in the same reset segment spanning at least fifteen minutes: normal-confidence slope
- stale samples older than the current refresh freshness window: stale-confidence and excluded from normal-confidence projection
- reset boundary crossed: segment history and do not treat reset replenishment as negative burn
- quota increase without reset: classify as provider anomaly or credit/refill event
- acceleration/deceleration: compare recent-segment slope to one-week slope and expose direction without hiding current hard quota limits

The algorithm must expose both:
- current remaining quota
- projected exhaustion time under active load when confidence is low or normal
- confidence label: unknown, insufficient, low, normal, stale

### Active Load Contract

The selector must account for running Codex activity before picking the next account.

Required model:
- proxy owns reservation handles and creates/releases them at request or WebSocket connection boundaries
- selector consumes active load summarized by account and route band
- reservation unit is an estimated percentage-point burn-rate contribution per active Codex stream for the relevant route band, not a persistent quota decrement
- HTTP and WebSocket requests may use different reservation weights, but both must be deterministic and test-covered
- reservation lifecycle covers WebSocket open, request start, response complete, error, close, and task cancellation
- per-account projected burn combines observed history with active reservation load
- release guarantees on drop/error restore selection projection
- stale reservation cleanup and observability must identify leaked reservations by safe account label and route band, never by secrets

The existing account hold/minimum-pinning logic is not enough. Holding reduces switching churn; reservation/load accounting estimates quota burn from active work.

### Selection Contract

The selection decision must consider:
- enabled account
- active credential availability
- known current 5h and weekly quota
- reset times
- reset credits as display data; routing use deferred unless explicitly implemented
- observed run-rate confidence
- active reservations
- projected time to exhaustion
- minimum account pinning/cooldown
- affinity/previous-response ownership where applicable

Weekly quota is a hard eligibility limiter. An account with 5h quota but no usable weekly quota is not usable unless reset/credit semantics explicitly prove it can be used.

## Requirement 2: Codex-Safe Quota Exhaustion

### Codex Compatibility Contract

Codex treats normal usage-limit/quota errors as terminal for a turn. Therefore the router must not rely on Codex seeing provider quota errors and retrying to another account.

The router must:
- avoid known exhausted accounts before opening upstream
- mark accounts exhausted/quarantined when provider quota errors are observed
- only surface account-out-of-limits to Codex when all enabled accounts with active credentials are exhausted or unavailable
- distinguish transport connection-limit recovery from usage quota exhaustion

Quota-error observation is a narrow exception to pass-through behavior. The router may inspect only recognized provider control/error envelopes needed for quota accounting and account quarantine. It must not parse arbitrary model payload text, mutate normal streamed model content, or infer quota state from ambiguous strings. Ambiguous or unknown upstream payloads are pass-through and must not quarantine an account.

Recognized quota observations:
- HTTP/SSE upstream status or error envelope that explicitly represents usage or quota exhaustion
- WebSocket provider error envelope with explicit `usage_limit_reached` or equivalent quota exhaustion code
- WebSocket provider error envelope with explicit `websocket_connection_limit_reached`, which is transport retirement and not quota exhaustion

If a request has already been committed upstream and an explicit usage-limit envelope arrives in-flight, the router records/quarantines that account. The implementation may hide/retry only if a deterministic Codex-compatible proof shows the turn completes on another account without sticky HTTP fallback. Otherwise the usage-limit result remains an unavoidable post-commit failure and must be mitigated by earlier projection/retirement.

### WebSocket Account Switching Contract

No mid-stream upstream account swap is allowed.

Account identity may change only at a new upstream connection boundary selected before forwarding that request frame.

Proactive connection retirement is allowed only as a controlled reconnect strategy, with proof that:
- Codex retries/reconnects successfully
- router selects a non-exhausted account on the new connection
- the turn does not surface `UsageLimitReached`
- Codex does not enter sticky HTTP fallback for the session
- concurrent sessions remain isolated

If an in-flight upstream stream emits `usage_limit_reached` after the router has already committed the request to that account, the implementation must prove whether the router can safely hide/retry it. If it cannot, the spec treats that as an unavoidable post-commit failure and requires future mitigation through earlier projection and retirement.

### All-Accounts-Exhausted Definition

All accounts are exhausted when no enabled account with active credentials has a usable, reserve, or explicitly accepted unknown-fallback state for the route band.

Unknown quota is not healthy. Unknown means probe/verify in the background and use only when the known pool has no usable candidate and policy permits fallback.

## Requirement 3: Sessions Command

### Command Contract

Add:

```text
codex-router sessions
codex-router sessions --scope cwd
codex-router sessions --scope worktree
codex-router sessions --scope any
codex-router sessions --provider any
codex-router sessions --provider current
codex-router sessions --provider <id>
codex-router sessions --source interactive
codex-router sessions --source all
codex-router sessions --source subagents
codex-router sessions --sort updated
codex-router sessions --sort created
codex-router sessions --list --format table
codex-router sessions --list --format json
codex-router sessions --last
```

Defaults:
- scope: worktree
- provider: any
- source: interactive
- sort: updated

Interactive mode opens an `inquire` picker, lets the user move, search/filter metadata, choose a row, and launches:

```text
codex --profile codex-router resume <SESSION_ID>
```

### Data Contract

The sessions command reads Codex local state metadata from `state_5.sqlite` read-only using SQLx. It must not use or extend rusqlite.

Allowed metadata:
- session/thread id
- created_at, updated_at, recency_at
- cwd/worktree
- source
- thread_source/subagent fields
- provider id
- model when present
- git branch/origin/sha when present
- archive flag

Prompt-derived title and preview fields are allowed only as sanitized, truncated human-facing picker/table labels. They must not be emitted in JSON by default, must not be searched unless explicitly enabled by a future spec, and must be tested with canary prompt text to prove raw prompt bodies do not leak. Default metadata search is limited to session id, cwd/worktree, provider id, source, thread source, and git branch.

Forbidden in V1:
- transcript-content search
- raw rollout response bodies
- base instructions
- dynamic tools
- auth/account/token files
- full prompt text in JSON output

### Scope Filters

- cwd: exact normalized current directory match
- worktree: current git worktree/repo root containment; outside git, falls back to cwd with a visible note
- any: no cwd/worktree filter

### Provider Filters

- any: no provider predicate
- current: the configured Codex provider used by `codex --profile codex-router`; if unavailable, fail with an actionable error
- `<id>`: exact metadata provider id match

### Source Filters

- interactive: CLI and VS Code top-level sessions, excluding exec, app-server, and subagents
- subagents: subagent/thread-source/parent-linked sessions
- all: all supported session sources

## Requirement 4: Quota Status UX

Quota status must keep the compact account-level table shape:
- at most one row per account, with multiline cells for 5h and weekly
- no account_id column in default human output
- show quota bars and percentages
- show reset time for 5h and weekly
- show reset credits available
- show routing decision
- show projected runout/burn-down where historical confidence is sufficient
- clearly label unknown, blocked, stale, and all-accounts-out states

The table must not pretend point-in-time pace is historical burn. If history is insufficient, say that directly.

## Requirement 5: E2E Quota Proof

The implementation must include deterministic E2E tests for Codex-account-shaped quota behavior:

- rate-limit event with 5h quota, weekly quota, reset timestamps, and reset credits reaches router state
- quota history is appended and retained
- quota status displays the same current/reset/credit data
- selector uses history, active load, and current quota to pick the next account
- selected account becomes exhausted before next request; router avoids it
- upstream `websocket_connection_limit_reached` reconnects without marking quota exhausted
- upstream `usage_limit_reached` marks account exhausted/quarantined
- Codex sees quota/usage-limit only when all router accounts are exhausted

The implementation must also provide a live-gated E2E path using a real logged-in Codex account. This live path is not CI-default because real quota state is external, mutable, account-specific, and may consume quota. It must require explicit opt-in for network/account use, exit clearly when credentials are unavailable, and support a refresh/status/selection dry-run mode that does not intentionally submit model-generating work. Any live WebSocket or generation step requires a separate explicit confirmation. It must print sanitized evidence:
- account label
- route band
- 5h remaining/reset
- weekly remaining/reset
- reset credits
- refresh source
- selected routing outcome
- no tokens, auth headers, cookies, or raw prompt bodies

## Proof Gate

Before claiming done:
- unit tests cover estimator/scorer/reservation/session filters
- SQLx SQLite integration tests cover history, retention, reset boundaries, and current snapshots
- proxy integration tests cover account selection, active load, WebSocket reconnect, usage-limit quarantine
- CLI tests cover sessions, quota status table/json, invalid args, and dependency guardrails
- deterministic E2E covers Codex-account-shaped quota/reset/reset-credit/rate-limit behavior
- live-gated E2E command is documented and run when credentials are available
- implementation-review-swarm reviews diff and proof chain
- dependency/boundary proof shows no new or extended rusqlite SQL access was added
