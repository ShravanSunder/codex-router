# Enabled OAuth credential upkeep requirements

Date: 2026-09-25
Identity: `oauth-credential-upkeep-requirements-2026-09-25`

## Purpose and current evidence

An enabled Router account should remain usable after days without client work or quota. Quota exhaustion must not defer its OAuth upkeep. The affected `askluna` account had a stored renewable credential and repeated `auth_error` after its recorded access-token expiry; the available status does not reveal the provider's refresh response, so the historical failure cause is unproven. The owner confirmed that their accounts use Router's isolated device login, not the optional external `auth.json` import path.

Current source already sends every enabled credentialed account through background quota refresh regardless of remaining quota, but the resolver attempts OAuth refresh only after recorded access-token expiry. The quota worker and proxy use distinct refresh locks, and a cancelled request can leave a blocking provider refresh running without its returned replacement being committed. These are current-system risks to correct, not claims about what caused `askluna`.

## Authorized outcomes

| ID | Need and observable outcome | Authority |
| --- | --- | --- |
| U-OU-01 | Every enabled account with a saved renewable credential receives proactive OAuth upkeep while Router is running, regardless of quota, active clients, routing preference, or whether background quota probing is disabled. A deliberately disabled account receives no upkeep until re-enabled. | Owner: all tokens should be kept alive for their requirements; only enabled accounts are included; quota exhaustion and idle time must not cause expiry. |
| U-OU-02 | One Router credential generation is renewed safely across proxy requests, background work, explicit CLI calls, and re-login. Concurrent callers and request cancellation do not repeat a successful or ambiguously completed rotation or discard a successful replacement; a confirmed unspent transient failure may retry after its deadline. | Necessary for U-OU-01 under the current independent caller locks and cancellable blocking refresh. |
| U-OU-03 | Safely retryable provider, rate-limit, and local failures are retried without silently marking a credential healthy; rejected, ambiguously spent, or missing renewal material is visible as requiring user action. Credentials and provider response bodies never appear in diagnostics. A known still-valid access token may serve while renewal retries; a known expired token may not. Unknown expiry is reported honestly and uses periodic renewal plus 401 recovery. | Owner's outcome and existing secret/fail-closed contracts; Astra high advisory review. |
| U-OU-04 | The explicit `--auth-json` CLI paths and `account import-codex-auth` command are removed. `account login` continues to use an isolated Codex device login and Router-owned credential storage. Existing saved credentials are not deleted by this CLI cutover. | Owner explicitly requested removal after clarifying that their accounts do not use the import path. |
| U-OU-05 | A quota endpoint's access-token 401 is given a bounded chance to recover through the current renewable credential before an account is disabled; a rejected refresh credential or repeated 401 is reported distinctly from quota exhaustion. | Required to distinguish access-token failure from unrecoverable login loss when fulfilling U-OU-01/03. |

## Limits

The current saved renewable credential is maintained; historical token generations and the same access-token string are not preserved. Provider revocation, a missing refresh token, a prolonged process or provider outage, or process termination between provider rotation and secure persistence can still require re-login. Preexisting externally copied credentials may have another refresh owner; this design does not silently modify that owner's store. Host restart and shutdown behavior is outside this change. No live provider refresh, production process replacement, merge, or release is part of design or local proof.

The explicit CLI cutover removes `account login --auth-json`, `account import-codex-auth`, and `live quota --auth-json`. The device-login implementation still parses the temporary `auth.json` produced by Codex; `live quota --profiles-root` retains its separate diagnostic behavior.

## Success boundary

With an enabled account whose quota is exhausted and no client traffic, a controlled multi-day run observes renewal before known access expiry, commits a returned rotating refresh token, and later reuses that replacement. Concurrent proxy and background callers cause one active provider refresh for a generation. A cancelled initiating request does not cancel the credential commit. Safely retryable failure remains visible and retries; terminal rejection or ambiguous rotation requests re-login without leaking secrets. Disabled accounts stay disabled and idle. All explicit `--auth-json` CLI entry points reject as unknown after the cutover.
