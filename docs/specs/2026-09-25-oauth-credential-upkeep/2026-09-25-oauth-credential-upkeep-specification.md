# Enabled OAuth credential upkeep specification

Date: 2026-09-25
Identity: `oauth-credential-upkeep-specification-2026-09-25`
Requirements: [Enabled OAuth credential upkeep requirements](2026-09-25-oauth-credential-upkeep-requirements.md)

## Domain and identity

| Entity | Same-instance rule and relationship | Observable states and invariant |
| --- | --- | --- |
| E-OU-01 Router account | The existing configured provider account, identified by its Router account ID, independently of quota, routing status, and credential generation. | `enabled` and `disabled` remain routing states; upkeep visits only enabled accounts. |
| E-OU-02 Current credential | The one active saved OAuth generation for an account. A successful renewal or re-login changes its generation while keeping the same account. | It contains an access token and optional renewable refresh token. Historical generations are not maintenance targets. |
| E-OU-03 Renewal attempt | One provider use of the current generation's refresh token, from background upkeep, request-time resolution, explicit quota work, or 401 recovery. | Attempts serialize. Successful or ambiguously completed use blocks reuse of the old token; a confirmed unspent failure may retry after its deadline. Initiating-request cancellation cannot abandon the result. |
| E-OU-04 Credential health | The visible maintenance result for one account and current credential generation. | `healthy`, `retrying`, `reauth_required`, or `unrefreshable`. A health result for an old generation does not govern a replacement. |

## Observable obligations

### S-OU-01 — Independent proactive renewal

While Router is running, an enabled account with a saved refresh token is checked for renewal independently of its quota balance, client count, selection rank, and background quota-probe switch. The first check starts on startup and ordinary checks target a 180-second cadence. For a token with more than 30 minutes of known lifetime, renewal is due by 30 minutes before expiry; for a shorter known lifetime, it is due halfway through that lifetime, with a wake scheduled by that earlier deadline. Successful renewal age also makes it due after four hours even when the access expiry is missing or farther away. Existing credentials without a recorded successful renewal time are due on the first check. The provider call occurs when due, not on every check. A slow or failed call may delay a later attempt; no fixed maximum observation age is claimed across provider outages.

Disabled accounts are skipped without changing their credentials or routing state. Re-enabling makes their current credential due for maintenance before expired access material can be used. If a saved account has no refresh token, status is `unrefreshable` and it is never represented as automatically kept alive.

### S-OU-02 — Single effective refresh authority

For a given Router account and active credential generation, at most one Router caller may use its refresh token with the provider at a time, including callers in separate Router processes. Before sending the refresh request, it chooses a successor secret slot proven unused under account authority and durably records that slot and current generation as in progress. An older orphaned successor slot cannot satisfy this claim. A waiting caller re-reads active generation and in-progress state after acquiring authority, uses an already committed replacement, or waits for recovery instead of repeating an unresolved use of the old token. Re-login may replace a generation without a stale renewal overwriting it. A refresh that begins for a request continues through provider response and a bounded local commit attempt even if that request is cancelled. An orderly in-process serving-runtime exit drains claimed work for at most 30 seconds. The shipped Host-managed `serve` process does not invoke this drain on SIGTERM; a routine Host restart can interrupt a rotation. If work is interrupted or the drain bound is reached, the persisted unresolved claim blocks old-token reuse after task or process exit. The next caller observes the committed replacement, a safe retry deadline, or an explicit terminal failure; no losing caller overwrites the winner's next-generation secret.

Process termination, including an external signal, after the provider rotates a token but before its response is securely persisted can lose the replacement when the in-process drain does not run or finish. On restart, an unresolved in-progress generation cannot reuse its old refresh token. If a securely staged replacement exists, Router may finish its local activation; otherwise it reports re-login required. Known expired access material is never sent. An access token with unknown expiry may be tried until a 401 triggers S-OU-04, and its status must not claim known validity.

### S-OU-03 — Health, retry, and secret-safe failure

Successful renewal advances the active generation, records a non-secret success time, and sets health to `healthy`. A definitively unspent transport failure or explicit provider rate-limit/temporary rejection that establishes no rotation sets `retrying`, preserves a known still-valid access token, and schedules a bounded retry with backoff from one minute to at most 30 minutes; rate-limit retry guidance may extend that bound when provided. A failure after the request may have been accepted, including an interrupted response, generic server error, or malformed success response, is ambiguous and cannot retry the old refresh token. Attempts for one account do not stop upkeep for other enabled accounts. A known expired access token is never sent while waiting.

An explicit provider refresh-token rejection or ambiguous provider outcome for the current generation sets `reauth_required` and stops repeating that generation until re-login replaces it. Missing refresh material sets `unrefreshable`. Local failure before provider use may retry safely. After a successful provider response, Router retains the replacement and retries local persistence without another provider call for at most 30 seconds. It releases account authority only after the provider operation has ended; if local commit still fails, the persisted in-progress claim remains unresolved and blocks other processes from reusing the old refresh token. A later recovery may activate a replacement securely staged in the claim's previously unused slot or require re-login with `rotation_commit_failed` if no claim-owned replacement was saved. A response held only in process memory is lost on process exit and cannot be recovered. Health is keyed to active generation, so successful re-login clears an old terminal state.

The quota/status JSON account row exposes `oauth_maintenance_state` (`healthy`, `retrying`, `reauth_required`, `unrefreshable`, or `unknown`), nullable `oauth_last_success_unix_seconds`, nullable `oauth_next_attempt_unix_seconds`, and nullable `oauth_failure_class`. An in-progress claim projects as `unknown` while recovery has not determined its outcome, including immediately after restart; status alone cannot tell a live rotation from a stranded claim. On the first upkeep or request-time recovery check, a claim-owned staged replacement is activated, or a missing staged replacement becomes `reauth_required`. Human account and quota status use those non-secret states and tell the operator when re-login is required once that outcome is known. Failure classes distinguish `transport_unspent`, `rate_limited`, `provider_temporary`, `provider_rejected`, `provider_outcome_ambiguous`, `malformed_response`, `local_persistence`, and `rotation_commit_failed`; none includes a token, raw response body, account email, or credential path.

### S-OU-04 — Access-token 401 recovery

When the quota endpoint returns 401, recovery carries the rejected credential generation into the shared refresh authority. If another caller has already activated a newer generation, the quota request retries once with that generation without another renewal. If the rejected generation is still current, a terminal state or unresolved in-progress claim forbids provider reuse, and an active retry deadline is respected; a forced 401 may start one renewal only when that generation has no terminal state or cooldown. The quota request then retries once with the resulting current access credential. An explicit refresh-token rejection or ambiguous outcome makes the credential `reauth_required`. A safe transient renewal failure leaves the enabled account in `retrying` and its quota evidence failed or stale under existing routing rules. A second quota 401 after successful renewal may disable only the generation actually used for that retry. A 401 alone is not evidence that a refresh token expired; unknown-expiry credentials use this same recovery path.

### S-OU-05 — Explicit auth-file CLI cutover

`account login` accepts the isolated device-login path and no longer accepts `--auth-json`; `account import-codex-auth` is no longer a command. `live quota` no longer accepts `--auth-json` and retains `--profiles-root`. Removed syntax fails as an unknown command or option and is absent from current help and usage documentation. Existing active Router credential generations remain intact. The device-login implementation may still parse the temporary auth file produced in its private Codex home; no explicit external auth-file path is accepted for account creation.

## Proof cases

| Requirement | Observation |
| --- | --- |
| U-OU-01 | Enabled, zero-client, exhausted-quota account renews across simulated days with quota probing disabled; disabled peer makes no OAuth request; missing-expiry fallback renews. |
| U-OU-02 | Concurrent proxy, worker, and CLI callers across process boundaries cause one rotation per generation; request cancellation and re-login races preserve the winning generation; an old orphaned successor is never mistaken for a new claim. |
| U-OU-03 | Transient, 429, rejected, malformed, and post-rotation persistence failures produce distinct secret-safe status and bounded retry or terminal action; persistent local failure and orderly in-process runtime exit honor the 30-second drain, while external process termination leaves old-token reuse blocked by the durable claim. |
| U-OU-04 | Removed CLI forms reject; device login and saved credentials continue to work; current help/docs have no explicit `--auth-json` usage. |
| U-OU-05 | First quota 401 renews and retries once; failed renewal and second 401 have distinct containment. |
