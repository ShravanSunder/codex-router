# codex-router Account Router UX Fix Implementation Plan

Date: 2026-06-25

Status: draft for one `plan-review-swarm` cycle

Source spec: `tmp/spec-workflows/2026-06-25-codex-router-account-router-ux-fix/account-router-ux-fix-spec.md`

Goal id: `2026-06-25-codex-router-account-router-ux-fix`

## Source Coverage

Loaded and planned from:

- `tmp/spec-workflows/2026-06-25-codex-router-account-router-ux-fix/account-router-ux-fix-spec.md`
- `tmp/spec-workflows/2026-06-25-codex-router-account-router-ux-fix/swarm-ledger.md`
- `tmp/workflow-state/2026-06-25-codex-router-account-router-ux-fix/details.md`
- current source anchors in proxy, CLI, state, tests, scripts, and manifests

The source spec received one review cycle. Accepted findings were folded into the spec before this plan.

## Goal

Make `codex-router` usable and correct as an account/OAuth router:

- no extra WebSocket payload validation
- no `FirstFrameTooLarge` release behavior
- pass-through traffic except account/auth routing
- explicit active-turn routing semantics
- cleaned installed-app CLI
- schema/state behavior is reproducible
- proof catches the exact bugs the user saw

## Non-Goals

- Do not change Codex.
- Do not disable WebSockets.
- Do not add router-owned retries, fallbacks, synthetic application responses, warmups, heartbeats, or mid-frame switching.
- Do not add a broad admin API unless implementation hits a proof blocker.
- Do not merge without explicit approval.

## Execution DAG

```text
gate 0: current-head baseline and red tests
  |
  +-- slice A: WebSocket account-router pass-through law
  |
  +-- slice B: active-turn reservation and routing/status semantics
  |
  +-- slice C: schema/default state-root hardening
  |
  +-- slice D: CLI command contract and UX cleanup
  |
integration gate 1: proxy + state + CLI compile and focused tests
  |
  +-- slice E: proof matrix and installed runtime harness fixes
  |
integration gate 2: installed binary smoke + three-Codex e2e proof
  |
plan-review-swarm fixes addressed
  |
implementation-review-swarm
  |
implementation-pr-wrapup: PR-ready, not merged
```

Parallelization:

- Slices A and B touch overlapping `websocket.rs`/selection behavior and should be integrated by one owner or serial within one worker.
- Slice C can run after A/B interfaces are named, but migration tests can be prepared independently.
- Slice D can run in parallel after command contract is frozen.
- Slice E depends on A-D because it updates proof rows and e2e expectations.

## Vertical Slice Cards

### Slice A: WebSocket Pass-Through Law

Source anchors: R1-R5, WebSocket Contract, Proof Expectations.

Behavior:

- Remove release `FirstFramePolicy`, `FirstFrameTooLarge`, `MalformedFirstFrame`, and whole-frame JSON validation as first-frame routing gates.
- Keep bounded top-level auth-smuggling detector.
- Extract only bounded routing/affinity metadata.
- Forward exact first frame unchanged.
- Ensure provider/upstream metadata observation is bounded and non-gating.

Likely write surface:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/server.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-proxy/src/local_auth.rs` if detector extraction must move or split
- `crates/codex-router-proxy/src/provider_error.rs`
- `crates/codex-router-proxy/src/lib.rs`
- `scripts/check-release-runtime-guardrails.py`
- `scripts/proof-matrix.sh`

TDD:

1. Add/flip failing tests that currently expect `FirstFrameTooLarge` or malformed whole-frame failure.
2. Add nested/prompt/tool auth-looking string tests that must pass through.
3. Add >1 MiB first-frame byte-identical forwarding test through real serve path.
4. Make tests pass by removing payload policy and introducing bounded metadata extraction.

Checkpoint:

- `cargo test -p codex-router-proxy websocket -- --nocapture` focused tests pass.
- Guardrail fails before fix and passes after fix for forbidden release payload policy.

Split/replan trigger:

- If bounded key-only detection cannot be implemented without whole-frame parsing, stop and return to spec.

### Slice B: Active-Turn Reservation And Selection Semantics

Source anchors: R6-R7, Quota And Routing Status Contract.

Behavior:

- Active load means active turn.
- Release active reservation on terminal response before slow affinity persistence.
- Re-reserve for later same-socket request-like frames using bounded request-start evidence.
- Keep account pinned for WebSocket lifetime.
- Ensure quota/status wording does not claim static persisted quota prediction is live selection.

Likely write surface:

- `crates/codex-router-proxy/src/websocket.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-selection/src/reservation.rs` if reservation API needs small extension
- `crates/codex-router-cli/src/quota.rs`
- tests in `crates/codex-router-proxy/src/lib.rs`

TDD:

1. Add slow affinity-recorder test proving active reservation releases before recorder unblocks.
2. Add same-socket two-turn fixture proving re-reserve on later request-like frame and zero active after each completion.
3. Add unknown post-completion data frame fixture proving pass-through without reservation or close.
4. Add quota output test proving wording uses `preferred by quota` or equivalent, not live `next`, when live server state is absent.

Checkpoint:

- focused proxy tests and quota renderer tests pass.

Split/replan trigger:

- If request-start evidence requires more payload interpretation than allowed, implement release-before-affinity only and mark re-reservation as blocked for design reconvergence.

### Slice C: Schema And State Root Hardening

Source anchors: R8, R10, Data / Schema Contract.

Behavior:

- One schema version maps to one table set.
- Decide current behavior for drifted `active_client_leases`.
- Prefer process-local active-turn tracking; do not persist active turns unless plan proof finds it necessary.
- Normal commands use `~/.codex-router` defaults.
- New/extended SQLite runtime work uses SQLx only.

Likely write surface:

- `crates/codex-router-state/src/sqlite.rs`
- state tests in `crates/codex-router-state/src/lib.rs`
- CLI default/root code in `crates/codex-router-cli/src/lib.rs`
- possibly test fixtures under `tests/` or crate test modules

TDD:

1. Add fixture test for fresh DB expected schema.
2. Add fixture test for drifted v7 DB with `active_client_leases`; expected behavior is either ignored extra table or migrated to new version, as implementation chooses.
3. Add tests proving normal command roots default to `~/.codex-router`.

Checkpoint:

- state tests pass.
- no new rusqlite query path added for new behavior.

Split/replan trigger:

- If old drifted DB recovery needs destructive cleanup, stop for explicit user approval.

### Slice D: CLI Command Contract And UX Cleanup

Source anchors: R9-R13, CLI Surface.

Behavior:

- Migrate touched normal CLI surface to clap-owned typed commands.
- Normal help exposes only user commands.
- Hide or advance token/live/proof/import/profile-write internals.
- `account login --label <name>` defaults to device auth.
- Normal login does not silently store plaintext secrets; either secure backend is implemented, or command fails clearly and plaintext is hidden/advanced.
- `account list` uses `comfy-table` and JSON format option.
- `quota` is the user-facing command: cached-first, refresh with `indicatif` spinner when interactive, rerender.
- `sessions` defaults to cwd and uses `--cwd`, `--checkout`, `--repo`, `--any`; picker shows useful two-line items; DB opens SQLx read-only without create.

Likely write surface:

- `Cargo.toml` and `crates/codex-router-cli/Cargo.toml` for `indicatif` if needed
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/account.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/sessions.rs`
- `crates/codex-router-cli/src/profile.rs`
- `crates/codex-router-cli/src/live.rs`
- CLI tests in crate modules
- smoke scripts if help/output checks exist there

TDD:

1. Add help snapshot/trycmd tests proving internal noise is hidden.
2. Add parse tests for `account login --label matches`.
3. Add account-list table/json tests.
4. Add quota cached-first/refresh-output tests with a fake refresh client or fixture mode.
5. Add sessions default/filter tests for cwd/checkout/repo/any and read-only `state_5.sqlite`.
6. Add picker formatting unit test for title-first two-line item.

Checkpoint:

- `cargo test -p codex-router-cli` focused tests pass.
- `cargo run -p codex-router-cli -- --help` matches normal help contract.

Split/replan trigger:

- If full clap migration is too large, migrate only touched normal commands in this goal and keep hidden legacy parser behind an internal compatibility adapter.
- If secure secret backend is too large, normal login must fail clearly rather than using plaintext by default.

### Slice E: Proof Matrix And Installed Runtime Proof

Source anchors: R14 and Proof Expectations.

Behavior:

- Create this goal's proof namespace and evidence root:
  `tmp/plan-workflows/2026-06-25-account-router-ux-fix/evidence/`.
- Update `scripts/proof-matrix.sh` rows so this goal's planned rows exist,
  run, and write receipts under this goal's evidence root instead of the
  2026-06-24 async-runtime root.
- Update guardrails so they fail on current HEAD for release
  `FirstFrameTooLarge`, release `FirstFramePolicy`, `MalformedFirstFrame`, and
  whole-frame first-frame `serde_json::from_slice::<serde_json::Value>` gates.
- Ensure installed Codex concurrent proof records redacted selected account tags from upstream Authorization, not just session/model correlation.
- Run current-head proof gates and write fresh artifacts.

Likely write surface:

- `scripts/proof-matrix.sh`
- `scripts/check-release-runtime-guardrails.py`
- `tests/smoke/*`
- `crates/codex-router-test-support/src/installed_codex.rs`
- evidence paths under `tmp/plan-workflows/.../evidence` only as generated artifacts

TDD:

1. Add proof rows first and verify they fail on current HEAD for the expected
   reasons:
   - `AR-WS-GUARD`: detects release first-frame size/JSON gates.
   - `AR-WS-PASSTHROUGH`: detects >1 MiB first-frame exact forwarding failure.
   - `AR-WS-ACTIVE-TURN`: detects missing same-socket re-reservation proof.
   - `AR-HTTP-PASSTHROUGH`: preserves HTTP/SSE pass-through guard.
   - `AR-CLI-UX`: detects noisy normal help/default command contract.
   - `AR-SESSIONS`: detects sessions cwd/root-filter/read-only DB behavior.
   - `AR-SCHEMA`: detects schema drift/default root regression.
   - `AR-E2E-ACCOUNTS`: detects missing selected-account-tag e2e proof.
2. Add negative guardrail canaries before removing forbidden code.
3. Add installed harness assertion that fails if three clients route to wrong account tag.
4. Add current-head receipt validation that rejects artifacts unless
   `git_head` equals current HEAD or a source-path freshness guard is present.

Checkpoint:

- `scripts/proof-matrix.sh AR-WS-GUARD AR-CLI-UX AR-SESSIONS AR-SCHEMA`
  fails before implementation for expected reasons, then passes after relevant
  slices.
- all required proof rows run at current HEAD and write to this goal's evidence
  root.
- redaction validator passes.
- installed binary smoke proves WebSocket without `CODEX_ROUTER_TOKEN`.
- three-Codex e2e/soak proves concurrent WebSocket account tags.

Split/replan trigger:

- If live e2e would consume meaningful user quota, use deterministic mock upstream for automated e2e and mark live OAuth proof as manual opt-in.

## Requirements/Proof Matrix

| Row | Requirement | Slice | Proof modality | Layer | Evidence source | Freshness guard | Red/green |
| --- | --- | --- | --- | --- | --- | --- | --- |
| P1 | No release first-frame payload cap/gate | A | unit + guardrail | U/G | proxy tests + structural script | current HEAD and guarded files changed | yes |
| P2 | >1 MiB legal WS first frame forwards unchanged | A | real serve mock upstream | I/S | upstream hash transcript | current HEAD | yes |
| P3 | top-level auth smuggling rejects, nested prompt/tool strings pass | A | unit + integration | U/I | detector tests | current HEAD | yes |
| P4 | whole-frame JSON validity not required | A | unit + integration | U/I | future/malformed payload tests | current HEAD | yes |
| P5 | response/provider observation non-gating | A | slow observer fixture | I | timing/transcript artifact | current HEAD | yes |
| P6 | active turn releases before slow affinity persistence | B | slow recorder fixture | I | reservation counters | current HEAD | yes |
| P7 | same-socket later request re-reserves pinned account | B | two-turn WS fixture | I | reservation/counter transcript | current HEAD | yes |
| P8 | quota output distinguishes static prediction from live selection | B/D | renderer tests | U/CLI | table/plain/json snapshots | current HEAD | yes |
| P9 | schema version/table set reproducible | C | DB fixture tests | I | SQLite schema assertions | current HEAD | yes |
| P10 | defaults use `~/.codex-router` | C/D | CLI parse/default tests | U/CLI | assert_cmd/trycmd | current HEAD | yes |
| P11 | normal help hides internal/test commands | D | help snapshots | CLI | help stdout | current HEAD | yes |
| P12 | `account login` defaults to device auth and secure policy | D | parse + behavior tests | U/CLI | command tests | current HEAD | yes |
| P13 | account list friendly table/json | D | renderer tests | CLI | table/json output | current HEAD | yes |
| P14 | quota cached-first refresh/rerender | D | fake refresh fixture | CLI/I | output + DB timestamps | current HEAD | yes |
| P15 | sessions default cwd and root filters | D | SQLx read-only fixture | I/CLI | fixture DB output | current HEAD | yes |
| P16 | installed Codex WS works tokenless | E | installed smoke | S | redacted transcript | current HEAD | yes |
| P17 | three concurrent Codex clients route to expected account tags | E | e2e/soak | E | redacted account-tag transcript | current HEAD | yes |
| P18 | fmt/clippy/tests pass | all | quality gates | Q | command output | current HEAD | no |
| P19 | implementation review accepts diff | all | review swarm | R | review report | post-proof HEAD | no |
| P20 | PR-ready not merged | all | PR wrapup | PR | PR/checks report | fresh remote state | no |

## Validation Gates

Baseline/red phase:

```text
cargo test -p codex-router-proxy websocket_first_frame
cargo test -p codex-router-cli sessions
cargo run -p codex-router-cli -- --help
scripts/proof-matrix.sh AR-WS-GUARD AR-CLI-UX AR-SESSIONS AR-SCHEMA
```

Focused implementation gates:

```text
cargo test -p codex-router-proxy websocket
cargo test -p codex-router-state
cargo test -p codex-router-cli
scripts/check-release-runtime-guardrails.py
scripts/proof-matrix.sh AR-WS-GUARD AR-WS-PASSTHROUGH AR-WS-ACTIVE-TURN AR-HTTP-PASSTHROUGH AR-CLI-UX AR-SESSIONS AR-SCHEMA
```

Quality gates:

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Smoke/e2e gates:

```text
cargo run -p codex-router-cli -- --help
cargo run -p codex-router-cli -- account list
cargo run -p codex-router-cli -- quota
tests/smoke/installed_codex_mock.sh
scripts/proof-matrix.sh AR-E2E-ACCOUNTS
three-client installed Codex WebSocket e2e/soak with mini model targets
```

Use cheap/mini Codex model targets for runtime e2e clients when model choice is configurable. Review agents remain `gpt-5.5`.

## Checkpoint Commits

Commit after these verified checkpoints:

1. Spec + plan artifacts after plan review is addressed.
2. Slice A/B proxy fix after focused proxy tests and guardrails pass.
3. Slice C/D state + CLI fix after focused tests and help smoke pass.
4. Slice E proof harness after e2e/smoke proof passes.
5. PR-ready wrapup.

Do not stage unrelated untracked tmp directories except this goal's workflow artifacts and generated evidence explicitly needed for proof.

## Risks

- Secure keychain backend may be larger than this CLI cleanup. If so, do not silently fall back to plaintext; fail normal login clearly and keep plaintext hidden/advanced.
- Same-socket re-reservation can drift into payload interpretation. Keep request-start evidence bounded and non-gating.
- Proof rows can false-green if they only check absence of old names. Include behavioral canaries and current-head freshness.
- `quota` live diagnostics can drift into admin API scope. Keep live diagnostics minimal or static-label-only unless proof requires it.

## Next Workflow

Run exactly one `shravan-dev-workflow:plan-review-swarm` cycle. Address accepted findings, then route to `shravan-dev-workflow:implementation-execute-plan`.

phase_result: complete
evidence: this implementation plan
recommended_next_workflow: shravan-dev-workflow:plan-review-swarm
recommended_transition_reason: The reviewed spec is mapped to vertical slices, proof gates, and execution order.
