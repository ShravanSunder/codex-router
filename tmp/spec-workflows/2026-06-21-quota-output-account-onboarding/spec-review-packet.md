# Spec Review Packet: Quota Output And Account Onboarding

Repo: /Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router
Branch/worktree: feature/initial-codex-router, dirty working tree with prior quota/live changes
Target artifact: tmp/spec-workflows/2026-06-21-quota-output-account-onboarding/quota-output-account-onboarding-spec.md
Coverage from controller: 321 lines read in chunks 1-140, 141-280, 281-321
Design stage: post-draft, pre-plan spec review

## User Goal

Make codex-router actually usable for real local Codex routing by adding router-owned OAuth/account setup and fixing quota status output. The user explicitly called out that a missing login/import UX is a huge requirement, and confirmed quota should refresh periodically in the background into SQLite so request-time routing does not block.

## Claims To Review

- Router-owned credential and account state is required; Codex/Prodex `auth.json` is only explicit import/proof input.
- `account import-codex-auth` is the first usable onboarding path; `account login` may be reserved/fail-closed if browser/device OAuth is too large for this slice.
- SQLite owns non-secret account metadata and quota snapshots.
- Secret store owns OAuth token material and local router token.
- Full logout requires `SecretStore::delete_secret`; blank-secret overwrite is not acceptable.
- Serving must read SQLite snapshots and must not block request-time routing on broad provider quota refresh.
- Provider quota refresh should run periodically in the background and persist snapshots to SQLite.
- Default quota output should be compact and decision-oriented.
- Detailed all-window quota output remains available for debugging.
- Pace wording should avoid `ahead`/`behind`; use burn/runway language.
- Live OAuth/quota proof remains explicit and approval-gated.

## Key Local Evidence

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:110` says quota snapshots refresh in background and request-time routing must not block on broad quota refresh.
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md:188` says `auth.json` is only compatibility input; normal serving/background quota/account selection/token refresh must read router secret-store.
- `crates/codex-router-cli/src/lib.rs:313` has no top-level account or quota namespace today.
- `crates/codex-router-cli/src/live.rs:192` fetches live quota but does not persist.
- `crates/codex-router-cli/src/live.rs:303` renders quota table inside live-fetch code.
- `crates/codex-router-cli/src/live.rs:521` currently renders `ahead`/`behind` pace wording.
- `crates/codex-router-state/src/account.rs:35` has account metadata.
- `crates/codex-router-state/src/quota_snapshot.rs:35` has reduced persisted quota snapshots.
- `crates/codex-router-secret-store/src/file_backend.rs:15` has only `write_secret` and `read_secret`.
- `crates/codex-router-secret-store/src/account_tokens.rs:8` has only upstream access-token key convention.
- `crates/codex-router-quota/src/worker.rs:23` defines scheduling without inline provider I/O.
- `crates/codex-router-proxy/src/http_sse.rs:607` selector reads accounts, secrets, and route-band quota snapshots at request time.

## Review Lanes

Each lane should return:

- lane name
- verdict: ready | needs revision | blocked | decision-needed
- accepted candidate findings
- contested tradeoffs
- open questions
- evidence paths or sections
- smallest spec edit
- proof or validation command
- confidence: high | medium | low

Do not implement code or edit files.
