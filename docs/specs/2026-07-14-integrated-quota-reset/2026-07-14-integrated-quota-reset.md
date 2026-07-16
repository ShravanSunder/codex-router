# Integrated Quota Reset

## Product intent

`codex-router quota` is the single interactive surface for understanding quota and acting on a
reset credit. The user focuses an account in the existing quota list and presses `Ctrl-R` to open a
live reset-detail mode in the existing detail pane. The account list, focus, responsive layout,
terminal lifecycle, and visual language do not change owners.

The reset detail is useful before an account is eligible to consume a credit. It always attempts to
show the focused account's live weekly usage and validated reset-credit inventory, including credit
status and expiration. Weekly remaining strictly below one percent gates the ability to select
`Yes` and consume; it does not gate inspection or opening confirmation.

Success means the workflow feels like another mode of the quota screen, makes data provenance
explicit, and makes accidental or wrong-account redemption structurally difficult.

## Product decisions

1. The standalone reset account picker and standalone confirmation render loop are removed.
2. `Ctrl-R` in quota browse mode enters reset detail for the currently focused stable account ID.
3. `Ctrl-R` or `Esc` before POST returns to quota browse and guarantees no consume POST was issued.
4. Reset inspection fetches live weekly usage and live credit details independently, regardless of
   eligibility. Partial read-only results remain visible while confirmation `Yes` stays disabled.
5. Live usage is the sole weekly eligibility authority. Live credit details are the sole credit
   inventory and selection authority. Persisted quota is display context only.
6. Confirmation starts on `No`. Only explicit selection of `Yes` followed by `Enter` authorizes
   revalidation and, if revalidation passes, one consume POST.
7. Once the consume POST is dispatched, application exit/cancel shortcuts do not claim
   cancellation. The screen waits for a known result or shows `Outcome unknown`.
8. `codex-router quota reset` no longer launches a workflow. It returns targeted, non-networking
   guidance to open `codex-router quota`, focus an account, and press `Ctrl-R`.
9. Returning from a result does not refresh or persist provider data. The quota screen states that
   persisted browse data may remain stale until the normal refresh path updates it.
10. Automated development, CI, review, and smoke proof never contact a real provider endpoint and
    never consume a real reset credit.
11. Completed inspection may always open confirmation. `No` remains selected by default; `Yes` is
    selectable only while fresh live weekly remaining is strictly below one percent and the fresh
    live inventory contains a usable earliest credit. Loading, failed, stale, or ineligible facts
    leave `Yes` visibly disabled with the reason shown.
12. Every live provider operation has an in-pane activity indicator naming the operation. Previous
    live facts may remain visible during refresh only when labelled as previous and refreshing;
    persisted SQLite facts remain separately labelled as saved.
13. The reset detail exposes the complete validated credit inventory. When it exceeds the detail
    viewport, `PgUp`/`PgDn` scroll only that inventory and the pane shows position plus remaining
    count; credits are never silently clipped.
14. Provider consume invocation is the conservative irreversible boundary. All fallible local
    preparation completes before invocation. After invocation begins, any failure without a
    validated provider outcome is `Outcome unknown`, even when the transport may have failed before
    sending bytes.
15. Revalidation compares a non-rendered fingerprint of the exact credential and routing bundle
    used for inspection. Replacing secret contents under the same generation therefore refuses
    before POST; generation equality alone is insufficient.
16. A successful revalidation creates one non-clone, single-use commit capability. Consuming it by
    value is the only operation that may invoke the provider POST.

## User-visible state contract

```text
Quota browse
  Ctrl-R(focused AccountId)
    -> Inspecting
         -> Inspected
              live weekly fact or error
              live credit inventory or error
              saved/live disagreement warning when applicable
              Enter -> Confirming
         -> Confirming (No selected; Yes enabled only by fresh eligible facts)
              No/Cancel -> Quota browse, zero POST
              disabled Yes -> remains Confirming, zero POST
              explicit enabled Yes + Enter -> Revalidating
         -> Revalidating
              account/generation/credential changed -> Refused, zero POST
              weekly not <1% -> Refused, zero POST
              earliest credit changed/expired -> Refused, zero POST
              all facts current -> Committing
         -> Committing
              one POST already authorized and dispatched
              known response -> Result
              ambiguous failure -> Outcome unknown
         -> Result
              Enter/Esc/Ctrl-R -> Quota browse
```

`Inspected` means both inspection GETs for the current correlated attempt have reached a terminal
success or failure state. `Enter` has no effect while either GET is still in flight. From
`Inspected`, `Enter` always opens confirmation, including when one fact failed or the account is
ineligible; eligibility controls `Yes`, not access to confirmation.

### Dynamic shortcuts

Browse:

```text
↑/↓ focus   ctrl-r inspect reset credits   esc/q exit   ctrl-c exit
```

Inspecting:

```text
esc/ctrl-r back   ctrl-c exit without consume
```

Inspected:

```text
enter confirmation   esc/ctrl-r back   ctrl-c exit without consume
```

Confirmation:

```text
←/→ select   enter confirm   esc/ctrl-r cancel   ctrl-c exit without consume
```

`No` is initially selected. `Yes` is visibly disabled and cannot receive selection while required
live facts are loading, failed, stale, or ineligible. The confirmation pane states the exact reason.

When the validated inventory exceeds the detail viewport:

```text
pgup/pgdn credits (1–4 of 9)   enter confirmation   esc/ctrl-r back
```

Revalidating:

```text
⟳ Revalidating account, live weekly usage, and selected credit…
```

Committing:

```text
⟳ Consuming reset credit… waiting for a definitive result.
```

The application does not describe `Esc`, `Ctrl-R`, or `Ctrl-C` as safe cancellation after POST
dispatch. Forced process termination remains outside the application's guarantees.

### TUI layout contract

The existing quota shell is immutable across workflow modes. Reset may replace only the ordinary
selected-account detail content and the mode-specific shortcut footer. The title, route summary,
account-list rows, focus marker, quota bars, colors, spacing, clipping, height budgeting, and
responsive thresholds remain render-equivalent to browse mode.

The wireframes specify information hierarchy, not fixed character widths. Existing viewport
budgeting and clipping still apply. Live reset content never becomes a second screen, modal account
picker, or independently focused account list.

At 160 columns and wider, reset detail occupies the existing sidecar detail placement:

```text
Quota status
Route summary

┌─ Accounts ─────────────────┐   ┌─ Reset details — askluna ─────────┐
│ ❯ askluna   quota bars    │   │ ✓ Live weekly remaining       0% │
│   matches   quota bars    │   │ ⟳ Refreshing reset credits…      │
│   ssdev     quota bars    │   │ Previous inventory — refreshing │
│                           │   │ Weekly reset · available         │
│                           │   │ expires 2026-07-18               │
└───────────────────────────┘   └────────────────────────────────────┘

esc/ctrl-r back   ctrl-c exit without consume
```

Below 160 columns, the same detail content replaces the ordinary selected-account detail below the
unchanged account list. It does not become a new screen or picker:

```text
Quota status
Route summary

┌─ Accounts ───────────────────────────┐
│ ❯ askluna   quota bars             │
│   matches   quota bars             │
│   ssdev     quota bars             │
└─────────────────────────────────────┘
┌─ Reset details — askluna ────────────┐
│ ✓ Live weekly remaining            0% │
│ ⟳ Fetching live reset credits…       │
│ Previous inventory — refreshing      │
│ Weekly reset · available · Jul 18     │
│ Enter opens confirmation              │
└─────────────────────────────────────┘

esc/ctrl-r back   ctrl-c exit without consume
```

After completed inspection, `Enter` opens confirmation even when the account is ineligible. The
confirmation remains default-No and explains why `Yes` is disabled:

```text
Consume one reset credit for askluna?

Live weekly remaining: 4%
Credit expires:         2026-07-18

Cannot consume: live weekly remaining must be strictly below 1%.

                         ❯ No       Yes (disabled)
←/→ select   enter confirm   esc cancel
```

## Requirements

### Existing quota screen ownership

R1. Interactive quota retains the existing title, route summary, account rows, quota bars, colors,
spacing, focus marker, selected detail placement, clipped-list behavior, height budgeting,
159/160-column stacked/sidecar transition, resize behavior, normal browse navigation, and terminal
lifecycle. Reset state may vary only the selected-detail content and mode-specific shortcut footer.
Returning to browse restores ordinary detail for the same surviving `AccountId`.

R2. There is exactly one interactive quota render loop and one account-selection owner.

R3. The quota footer documents `Ctrl-R` in browse mode. Starting reset does not clear the screen,
open another picker, or print workflow messages below the TUI.

R4. Plain, JSON, non-terminal, and static table quota output remain non-interactive and perform no
reset network I/O.

### Stable identity and reload behavior

R5. Every interactive quota row carries a non-rendered stable `AccountId` and active credential
generation. Labels and row positions are display/navigation data, never reset authority.

R6. Semantic focus is stored by `AccountId`; render index is derived after each persisted reload.
Duplicate labels, reorder, insertion, and removal cannot retarget focus or a reset attempt.

R7. `Ctrl-R` snapshots `AccountId`, credential generation, and a unique UI attempt generation.
Every async completion carries and must match those correlation values before changing UI state.

R8. While reset mode is active, persisted reload may update the surrounding quota shell but cannot
replace the pinned reset pane or target. A missing/disabled account or changed credential generation
invalidates confirmation and requires a fresh inspection.

### Live inspection and provenance

R9. Inspection issues a live usage GET and a live credit-details GET for only the focused account.
It does not eagerly inspect every account.

R10. Inspection fetches both facts regardless of weekly eligibility. If one GET succeeds and the
other fails, the successful read-only fact remains visible, the failure is explicit, and
`Yes` remains disabled.

R11. The detail pane shows validated credit title, status, expiration/non-expiring state, and only a
redacted credit identifier. Available credits are ordered by finite expiration ascending, then
non-expiring credits. The earliest usable credit is highlighted. The complete safe validated
inventory is discoverable: overflow uses deterministic `PgUp`/`PgDn` scrolling with visible range
and remaining count, never silent clipping or an earliest-credit-only summary.

R12. A credit marked available but already expired at the current clock is not usable. Unknown
statuses, invalid timestamps, empty identifiers, control characters, and malformed data fail
closed.

R13. Persisted quota summary and live reset detail remain separate, source-labelled observations.
Neither overwrites the other. When counts disagree, both values and a warning are rendered; live
credit detail governs selection.

R14. Completed inspection may enter confirmation regardless of eligibility. The reducer derives
`YesEnabled` only from fresh results belonging to the current correlated attempt: live weekly
remaining is strictly below one percent, the live credit GET succeeded, and the inventory contains
a usable earliest available credit. With whole-percent provider data, only zero percent passes.
Any loading, refreshing, failed, stale, missing, or ineligible authority fact derives
`YesEnabled = false` and a visible reason. Previous results never enable `Yes`.

### Confirmation and consume authority

R15. Confirmation displays the pinned account label and redacted tag, live weekly value, selected
credit title/hint/expiration, and an explicit scarce-credit warning.

R16. `Enter` from `Inspected` always opens confirmation. Confirmation starts with `No` selected.
Enter while `No` is selected returns to quota browse with zero POSTs. `Enter` during `Inspecting`
has no effect. Repeated Enter or transition keys cannot create duplicate confirmation,
revalidation, or consume effects.

R16a. Disabled `Yes` cannot receive focus or authorization. If `Yes` was selected and any required
fact becomes loading, refreshing, stale, failed, missing, or ineligible before Enter is accepted,
selection returns to `No` and no revalidation or POST begins.

R17. Explicit `Yes` authorizes revalidation, not a blind POST. Before POST, the workflow re-reads
account enabled state and active generation, loads exact credentials read-only without refresh,
rechecks expiry, compares the exact non-rendered credential/routing fingerprint used for inspection,
refetches live weekly usage and credit inventory, and requires the same earliest usable credit and
confirmation fingerprint. The fingerprint binds `AccountId`, active generation, ChatGPT routing
identity, access-token bytes through a one-way in-memory digest, credential expiry, live weekly
remaining, selected credit ID, status, title, expiration/non-expiring state, and any displayed hint.
Refresh-token/source metadata is outside reset authority because it is not sent to the provider. Raw
secrets and the fingerprint never enter presentation state, logs, errors, or persisted storage.
Revalidation requires exact fingerprint equality, requires the selected credit still to be the
earliest usable credit, and rechecks credential/credit expiry at the commit clock. Changes only to
later non-selected credits do not invalidate an otherwise identical confirmation.

R18. Any pre-POST cancellation, stale attempt, missing account, changed generation, credential
error, weekly refusal, or changed/expired credit proves zero POSTs and may state that no reset was
consumed.

R19. One confirmed logical attempt owns one opaque redeem request ID. Successful revalidation may
mint exactly one non-`Clone`, non-serializable `CommitCapability` that binds the attempt generation,
confirmation fingerprint, exact pinned routing identity, selected credit ID, and redeem request ID.
The capability is owned by the workflow service, never presentation, and `consume(capability)` takes
it by value. No other API can invoke consume. Repeated keys, duplicate effect delivery, or stale
completion therefore cannot dispatch a second POST for the logical attempt. The redeem request ID
is minted only when successful revalidation creates this capability, never during read-only
inspection.

R20. All credential/account checks, request validation, and body serialization complete before
`consume(capability)` invokes the provider port; failures there are typed precommit failures with
zero POST. Immediately before provider invocation the reducer enters `Committing`, and invocation is
the conservative irreversible boundary. The consume port returns only `Known(typed response)` or
`OutcomeUnknown(sanitized reason)` after invocation; transport errors, non-2xx responses, timeout,
response loss, body-read failure, oversized body, malformed JSON, and unknown response code all map
to `OutcomeUnknown` unless a validated typed provider outcome exists. It never reports `not
consumed`, exposes raw bodies, or retries automatically.

R21. Successful 2xx responses with known provider codes (`reset`, `nothing_to_reset`, `no_credit`,
`already_redeemed`) render as distinct typed outcomes. Unknown codes, non-2xx responses, malformed
or oversized bodies, and response/transport failures after invocation fail closed as `Outcome
unknown` with sanitized bounded diagnostics.

### Async and lifecycle ownership

R22. The top-level Tokio runtime natively owns the complete quota CLI command entry. Status in every
format, explicit refresh, interactive reset, and legacy migration guidance dispatch through async
quota command composition. The interactive path awaits one iocraft render loop. No production
`QuotaCommand` call graph creates a nested Tokio runtime, calls `block_on`, or spawns an OS thread
merely to wrap async quota work. Pure parsing, projection, formatting, reducer, and render
calculations remain synchronous functions called from the async entry. The serve-owned background
quota-refresh worker is not a `QuotaCommand` path; its runtime ownership and behavior remain
unchanged and unreachable from quota CLI dispatch.

R23. All quota-command SQLite and provider operations are awaitable. Bounded blocking secret-store work
uses `spawn_blocking`; no database connection, transaction, mutex, credential lease, or blocking
task spans terminal confirmation or provider I/O. Account-authority reads open SQLite read-only and
query-only with `busy_timeout(0)`, request no write transaction or RESERVED/PENDING/EXCLUSIVE lock,
and perform no busy-handler retry. If a coherent read transaction cannot begin immediately, they
return a typed refusal. Each fresh transaction observes the latest committed state visible when it
begins. Normal SQLite WAL/SHM reader coordination is allowed; reset performs no SQL mutation,
migration, checkpoint, refresh, or application-owned persistence. Immutable/nolock modes are
forbidden against the live database.

R24. Terminal input, resize, spinner, and persisted reload remain responsive during live requests.
Cancelling pre-POST invalidates the attempt generation; late or out-of-order completions are ignored.

R24a. The reset detail pane renders a distinct, continuously visible activity row for each provider
operation: inspection live-usage GET, inspection credit-inventory GET, revalidation live-usage GET,
revalidation credit-inventory GET, and consume POST. Each row names the operation and exposes its
semantic state: not started, loading/refreshing, succeeded, failed, cancelled, or, for consume,
request dispatched and awaiting outcome. Meaning cannot depend on animation or color alone.

When an operation starts again, its last successful value may remain visible only beneath an
explicit `previous result — refreshing` label. That value is informational, is not current
authority, and cannot enable `Yes` or authorize POST. A refresh failure leaves the old value
labelled previous and renders the current failure separately. At the POST commit point, the consume
indicator becomes `request dispatched — awaiting definitive outcome`; ambiguous failure then
follows R20. Persisted SQLite values remain labelled `saved`, never current or previous live.

R25. The interactive quota command's effect supervisor, not a render-helper hook, owns effect task
handles. Component teardown cancels or invalidates read-only/pre-POST effects. When the capability
is consumed, ownership of the consumptive task transfers to the command-level supervisor; mode
changes, resize, component teardown, and ordinary cancel keys neither abort nor detach it. The
supervisor awaits a known result, bounded-time unknown outcome, or forced process termination, then
drops authentication and authority values without persistence. Forced process or terminal loss is
outside application control and cannot create retry or `not consumed` claims.

## Technical contract

### Spec boundary / separability map

```text
quota command / persisted projection
  owns: SQLite-derived status, ordering inputs, stable row identity
  exposes: async persisted view source + static render adapters
                     |
                     v
QuotaStatusComponent
  owns: one TUI, AccountId focus, responsive shell, key routing
  holds: the latest immutable render-safe reset snapshot
                     |
          typed intents / snapshots
                     v
QuotaInteractiveSession
  owns: sole reducer, attempt correlation, authority, and effect task handles
  exposes: bounded intent sender + immutable redacted snapshot receiver
                     |
          non-spawning operation futures
                     v
ResetWorkflowService
  owns: inspection/revalidation validation and single-use commit execution
  exposes: non-spawning operation futures; owns no task handles or presentation state
                     |
          read-only state/secrets + provider port
                     v
quota_reset credentials / domain / provider
  owns: exact credential binding, pure policy, validated HTTP protocol
  exposes no secrets or raw provider payloads upward
```

### Permitted dependencies

- Quota projection may construct identity-bearing presentation DTOs.
- Quota presentation may invoke typed reset workflow effects and render typed/redacted outcomes.
- Reset workflow may use read-only credential/state ports, pure reset domain policy, and the live
  provider port.
- Reset render helpers may live beside quota presentation but own no render loop, focus, provider,
  or credential access.

### Forbidden dependencies

- Presentation must not import bearer-token, secret-store, raw HTTP, or raw provider payload types.
- Reset workflow must not depend on row index, terminal width, iocraft hooks, or persisted quota as
  authority.
- Persisted reload must not mutate or replace live reset state.
- `quota_reset` must not own a standalone picker or render loop.
- CLI dispatch must not own reset transitions beyond dependency composition and async entry.

### Identity types

Three identities remain distinct:

- Active credential generation selects the credential bundle authorized for an account.
- UI attempt generation rejects stale async completions.
- Redeem request ID identifies one logical provider consume attempt.

They are never substituted for one another.

### State and effect separation

The reset workflow domain module owns a pure typed reducer/state machine, not presentation and not a
collection of hook-local booleans. Presentation converts keys into intents and renders immutable
redacted snapshots. One command-owned `QuotaInteractiveSession` owns the reducer instance,
authority-bearing values, attempt correlation, and all reset task handles. It asks the non-spawning
workflow service for operation futures and reduces their correlated outcomes. The session picker's
`use_async_handler` plus identity check is the nearby repository pattern; this design does not
introduce a generic event bus or global store.

Inspection, revalidation, and consume are distinct effects so cancellation and the POST commit point
remain observable. Provider/auth objects do not enter the reducer's renderable state.

Every effect request carries `AccountId`, expected active credential generation, UI attempt
generation, and a unique operation generation. Every result repeats those fields plus the operation
kind and one typed terminal state. Inspection usage and credit results are separate envelopes, so
either may update its own activity row without waiting for the other. The reducer accepts a result
only when all correlation fields match the current attempt and operation; stale, duplicate, or
out-of-order results are ignored. Presentation's generation is an expected-version snapshot only;
fresh read-only account state is authoritative during revalidation.

### Interactive dispatch matrix

Only status with effective table format and both stdin and stdout attached to terminals enters the
native async quota render loop and constructs reset dependencies. The existing top-level Tokio
runtime awaits that loop directly.

| Command surface | Terminal condition | Execution contract |
| --- | --- | --- |
| `quota` table | stdin TTY and stdout TTY | one async interactive quota loop; reset available |
| `quota` table | either stream non-TTY | existing static table fallback; no reset dependencies |
| `quota --plain` | any | async quota entry invokes pure plain writer; no reset dependencies |
| `quota --json` | any | async quota entry invokes pure JSON writer; no reset dependencies |
| quota refresh/help | any | async quota entry preserves behavior; no reset workflow construction |
| `quota reset` | any | async quota entry invokes pure migration guidance; no state, secret, or network access |

Process-independent dispatch tests inject both terminal booleans; production dispatch uses the same
predicate. Here `pure`/synchronous describes calculation or output functions, never top-level quota
command ownership. Static/plain/JSON/help/guidance paths do not construct provider, credential, or
reset workflow dependencies.

### Legacy `quota reset` compatibility

`codex-router quota reset` remains parseable with no reset-specific options and performs no
filesystem access. It writes exactly the following line plus a trailing newline to stdout and exits
zero regardless of TTY state:

```text
Quota reset moved to codex-router quota: focus an account and press Ctrl-R.
```

It writes nothing to stderr. `quota reset --help` exits zero through the existing help path and
labels the subcommand as migration guidance, not an interactive reset API. All other arguments
retain the ordinary parser error contract and exit status 2.

### Provider composition and bounds

Installed production composition constructs the provider only with the fixed
`https://chatgpt.com/backend-api` origin and accepts no origin from CLI arguments, environment,
configuration, persisted state, or provider responses. Redirects are disabled. Each response body
is limited to 1,048,576 bytes: an oversized declared `Content-Length` is rejected before collection,
and missing/incorrect/chunked lengths are streamed only through limit-plus-one bytes. Overflow stops
reading and produces a typed failure. Diagnostics may contain only operation kind, safe error class,
HTTP status when known, size-limit classification, and redacted account/attempt tags. They never
contain authorization, ChatGPT routing identity, full router account/credit/redeem IDs, headers, raw
body, credential fingerprint, arbitrary provider strings, or unsanitized transport/parser errors.

Automated proof uses a composition-only fake/loopback provider seam compiled for test harnesses and
unavailable to installed behavior. Full-workflow/PTY proof fails closed before credential lookup or
request construction unless it receives an isolated fixture state/secret root and an assigned
loopback listener. A test egress guard rejects every non-loopback destination. Production origin
constants and ambient home credentials are structurally unavailable to the automated workflow
composition.

The dedicated feature-gated PTY executable has its own sealed harness argument contract. It accepts
only an explicit absolute isolated fixture root and the numeric `SocketAddr` of a listener that the
parent test already bound and retains on a loopback address. It does not accept a URL, hostname,
path, userinfo, query, fragment, environment override, or home-directory fallback. After validating
those harness-only inputs, it synthesizes ordinary `codex-router quota --router-root <fixture>`
arguments and passes them through the real CLI parser and composition-parameterized async quota
dispatcher. The harness-only arguments and loopback factory are absent from the installed
`codex-router` parser, main entry, and production reset factory, including all-feature builds. Both
wrappers converge on the same status loader, async quota session, supervisor, reducer, presentation
component, and iocraft render loop.

## Security context

Assets are OAuth credentials, provider account-routing identity, reset-credit identity, redeem
request identity, persisted router state, and scarce reset credits.

The consume authority chain is:

```text
stable focused AccountId + credential generation
  -> exact read-only credential and ChatGPT routing identity
  -> live weekly usage + live credit inventory
  -> default-No explicit Yes
  -> fresh account/credential + usage + credit revalidation
  -> one exact POST with one redeem request ID
```

Persisted quota, account label, row position, previous live facts, and the existence of a credit do
not grant consume authority.

Production composition uses the fixed provider origin, disables redirects, bounds request time and
response body size, validates response status/shape, and never retries a consume automatically.

## Non-goals

- No credential refresh, rotation, repair, or secret persistence.
- No persistence of live usage, credit inventory, confirmation state, or reset outcome.
- No non-interactive, forced, threshold-overridden, scripted, or multi-account redemption.
- No eager live inspection of all accounts.
- No automatic reconciliation/retry after an unknown POST outcome.
- No background quota-refresh behavior redesign or async rewrite outside the quota command family.
- No reusable application-wide TUI router, event bus, or global workflow store.
- No production-router restart/replacement or change to writer ownership.
- No real-provider automated proof and no real reset consumption during implementation or review.

## Proof expectations

The implementation plan must operationalize these proof modalities without using real endpoints:

1. Pure reducer/domain proof for every state/key transition, strict eligibility, expired-credit
   exclusion, deterministic ordering, inspected-to-confirming Enter, default No, disabled-Yes
   non-selection, and commit classification.
2. Deterministic iocraft tests for `Down, Ctrl-R`, clipped focus, duplicate labels, reload reorder,
   account removal, generation change, cancellation, delayed/out-of-order completions, repeated
   Enter/Ctrl-R, and stale attempt suppression. Controllable held futures prove keys, resize,
   independent activity rows, and cancellation remain responsive while each provider effect waits.
3. Normalized golden/differential text/ANSI captures proving browse render equivalence and
   browse-reset-browse round-trip restoration, plus captures for inspecting, independent partial
   inspection, previous-result-refreshing labels, disagreement warning, ineligible confirmation
   with disabled Yes, eligible default-No confirmation, Yes selection, revalidating, committing,
   success, provider no-reset outcomes, pre-POST failure, and outcome unknown at existing narrow/
   stacked/sidecar boundaries. Captures cover each of the five provider operations in flight and
   prove that state meaning does not rely on spinner animation or color.
4. Fake-provider ledgers proving inspection calls both GETs regardless of eligibility, every
   refusal/cancel/error before commit has zero POSTs, confirmed revalidation uses current account and
   exact credit, activity transitions follow request start/completion order, one logical
   confirmation produces at most one POST, and a consumed capability cannot be reused.
5. Loopback HTTP proof for exact paths/headers/body, redirect refusal, bounded bodies, malformed
   data, all typed consume codes, and conservative ambiguous failure after provider invocation.
   Boundary cases cover under/over 1,048,576 bytes, non-2xx, unknown code, connection refusal after
   invocation, close after request bytes, body-read failure, timeout, and malformed response.
6. Unique per-run temporary state/secret roots proving each fresh query-only SQLite transaction
   observes the latest committed WAL state visible when it begins, performs no busy-handler retry or
   write transaction/lock request, and returns a typed refusal when a coherent read cannot begin;
   plus non-creating secret access, strict secret-root
   byte immutability, expiry/generation/status changes, same-generation secret replacement, exact
   account routing, and no credential refresh/write. SQLite-owned WAL/SHM reader coordination is
   permitted; logical tables, schema, and application-owned state remain unchanged. Automated runs
   neutralize ambient home/router configuration, use fixture-only credentials, allocate unique
   loopback ports, and are safe across parallel tests and Worktrunk checkouts.
7. Async ownership proof that the complete quota CLI command family runs under the existing top-level
   runtime, with one interactive render loop, no nested runtime or `block_on`, no thread wrapper for
   `QuotaCommand` async work, no blocking provider work, and no stale task authority after exit. The
   serve-owned background quota-refresh worker is structurally unreachable from this assertion and
   retains its existing behavior.
8. Structural/cutover proof that the standalone picker/render loops are unreachable and
   `quota reset` guidance performs no state, credential, or network access. Import/dependency checks
   keep secrets/raw HTTP out of presentation and keep production-origin composition out of tests.
9. A compiled-binary PTY smoke under the top-level runtime enters interactive quota from isolated
   fixtures, focuses an account, sends `Ctrl-R`, observes live reset activity through loopback-only
   composition, returns to browse with zero POST, exits cleanly, and proves terminal restoration.
   It uses bounded event/protocol waits and fails on nested-runtime panic, output below the TUI,
   duplicate render loops, ambient credential lookup, or non-loopback egress.
10. A requirements/proof matrix in the implementation plan maps every R1–R25 obligation and each
    product/CLI/security contract to unit, component, integration, PTY smoke, structural, quality,
    and PR gates. Any not-applicable layer requires an explicit reason.

## Accepted tradeoffs

- Carrying stable account identity/generation in quota presentation DTOs adds non-rendered coupling;
  it is required to prevent wrong-account actions.
- A conservative refusal after account removal or credential rotation may inconvenience the user;
  the cost belongs to operator convenience rather than scarce-credit safety.
- Persisted and live values may disagree visibly. The design preserves provenance instead of
  pretending immediate consistency or writing state.
- The client can minimize but not eliminate the race between final GET revalidation and provider
  POST. Provider-side atomicity is outside this contract.
- Outcome unknown is less satisfying than an automatic retry, but it avoids false assurance and
  duplicate-redemption risk without documented recovery semantics.

## Superseded behavior

This spec is the complete normative contract. The 2026-07-13 guarded live reset plan is
non-normative provenance only and must not supply missing requirements or implementation decisions.
Its standalone account selection, standalone confirmation, `Ctrl-R` cancellation, and separate
`quota reset` async workflow are explicitly superseded.

## Planning inputs

The planner must map the resolved contracts to files, task order, red/green proof, and authoritative
commands. It may choose internal type and module names that preserve the ownership boundaries, but
it may not reopen the product flow, response bound, confirmation fingerprint, commit boundary,
inventory reachability, dispatch matrix, or no-real-provider harness contract.
