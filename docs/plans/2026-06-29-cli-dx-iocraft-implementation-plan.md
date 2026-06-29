# CLI DX iocraft Implementation Plan

Date: 2026-06-29

Status: reviewed for implementation

Source spec:
- `docs/specs/2026-06-29-cli-dx-iocraft-layout-spec.md`

Supporting evidence:
- `tmp/research-workflows/2026-06-29-iocraft-cli-dx/research-ledger.md`
- `tmp/spec-workflows/2026-06-29-cli-dx-iocraft/spec-review.md`
- `tmp/spec-workflows/2026-06-29-cli-dx-iocraft/spec-delta-review-2026-06-29.md`
- `tmp/plan-workflows/2026-06-29-cli-dx-iocraft-implementation/lanes/codebase-boundary.md`
- `tmp/plan-workflows/2026-06-29-cli-dx-iocraft-implementation/lanes/validation-proof.md`
- `tmp/workflow-state/2026-06-29-cli-dx-iocraft-implementation/details.md`

## Goal

Implement the accepted codex-router CLI DX redesign with `iocraft`, preserving
the router's core purpose: explain and control account routing without wide
tables, duplicate quota output, session-picker clutter, or logs corrupting UI.

## Non-goals

- No quota/account-selection algorithm changes.
- No auth/OAuth changes.
- No websocket routing changes.
- No persistence schema changes unless a reviewed follow-up explicitly expands scope.
- No public command rename from `codex-router sessions`.
- No production router port `8787` in runtime tests.
- No merge; PR-ready proof only.

## Source Coverage

- Spec read: `docs/specs/2026-06-29-cli-dx-iocraft-layout-spec.md`, 402 lines.
- Current session filters checked:
  - `SessionsRoot`: `cwd`, `checkout`, `repo`, `any`
  - `SessionsProvider`: `any`, `current`, provider id
  - `SessionsSource`: `interactive`, `all`, `subagents`
- Current quota renderer checked: `write_quota_table`, `write_quota_plain`,
  `write_selector_summary_table`, and auto-refresh flow.
- Current telemetry checked: `init_from_env` installs stderr fmt layer before
  command dispatch.
- Current test fixtures checked: session state fixtures, filter tests, picker
  injection tests, dependency-contract tests, quota formatting tests.
- CI proof checked: `.github/workflows/ci.yml` runs `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo nextest run --workspace`, `cargo deny check`, and `cargo audit`.

## Likely Write Surfaces

- `Cargo.toml`
  - add workspace `iocraft`
  - remove or stop using `inquire` if no longer needed after cutover

- `crates/codex-router-cli/Cargo.toml`
  - add `iocraft.workspace = true`
  - update dependency contract according to the spec

- `crates/codex-router-cli/src/lib.rs`
  - classify commands before telemetry initialization
  - pass command/log mode into telemetry setup
  - update tests for dependency contract, UI quietness, and command modes

- `crates/codex-router-cli/src/telemetry.rs`
  - split OTEL export from terminal log mirroring
  - drop CLI UI logs when OTEL is absent
  - keep server terminal lifecycle/debug behavior intentionally scoped

- `crates/codex-router-cli/src/quota.rs`
  - replace legacy 11-column human table with account-row presentation model
  - keep JSON contract
  - keep deterministic non-TTY/plain output
  - change refresh behavior to avoid duplicated human blocks

- `crates/codex-router-cli/src/sessions.rs`
  - replace `inquire::Select` with an `iocraft` picker boundary
  - expose root/provider/source filter state in picker model
  - add interaction model for filter switching
  - keep list/json and last/dry-run behavior

- new CLI presentation modules, likely under `crates/codex-router-cli/src/`
  - `terminal_ui.rs` or `presentation/`
  - `quota_view.rs`
  - `sessions_view.rs`
  - shared truncation/width helpers

## Requirements / Proof Matrix

| Requirement | Source | Owning Slice | Proof Modality | Layer | Evidence Source | Freshness Guard | Red/Green |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `quota status` renders account rows, not 11-column table | spec Quota Status Contract | S2 | golden render snapshots at wide/medium/narrow widths | unit/golden | `cargo test -p codex-router-cli ...quota...` | after S2 | yes |
| quota refresh does not append duplicate tables | spec Refresh behavior | S2 | refresh success/failure tests with TTY/non-TTY modes | unit/integration | targeted cargo test | after S2 | yes |
| JSON quota output remains unchanged | spec Output Modes | S2 | existing/new JSON contract tests | unit | targeted cargo test | after S2 | yes |
| session picker shows root/provider/source filters | spec Session Picker Contract | S3 | mock-terminal render test | unit/interaction | targeted cargo test | after S3 | yes |
| session picker can switch each filter dimension | spec Filter behavior | S3 | simulated key events through `iocraft` mock terminal for root, provider, and source | unit/interaction | targeted cargo test | after S3 | yes |
| session picker fits terminal width | spec Small-width/ultra-narrow behavior | S3 | mock-terminal width cases for wide details, narrow one-line filters, ultra-narrow stacked filters, middle truncation, and terminal-too-narrow exit | unit/golden/interaction | targeted cargo test | after S3 | yes |
| non-TTY interactive sessions fail concisely | spec Output Modes | S3 | run_with_io/non-TTY test | unit | targeted cargo test | after S3 | yes |
| `sessions --list --format table` respects output modes | spec Output Modes | S3 | TTY human rows, non-TTY deterministic plain text, quiet stderr | unit/golden | targeted sessions tests | after S3 | yes |
| CLI UI logs go to OTEL or are dropped, never UI streams | spec Logs and Telemetry Contract | S4 | unit tests around telemetry mode + smoke command stderr/stdout assertions | unit/smoke | targeted test + smoke transcript | after S4 | yes |
| server logs prefer OTEL and terminal output is intentional | spec Server behavior | S4/S5 | telemetry mode tests + debug-port serve smoke | unit/smoke | targeted test + smoke transcript | after S5 | yes |
| version/build identity appears on quota/status human banners only | spec Version Contract | S2 | formatter tests + JSON/telemetry/`--version` version contract test | unit | targeted cargo test | after S2 | yes |
| no secrets/raw auth/rendered logs leak into UI | spec Privacy/Security | S2/S3/S4 | negative assertions in UI output tests | unit/smoke | targeted test + smoke transcript | after each slice | yes |
| runtime proof never uses port `8787` | goal/spec | S5 | command transcript shows explicit debug port != 8787 | smoke/e2e | smoke transcript | every runtime proof | no red needed |

## Telemetry and Terminal Output Policy

This policy must be implemented before changing the UI renderers. It separates
structured telemetry export from terminal log mirroring.

Definitions:
- OTEL export means spans/metrics/log-relevant telemetry sent through the
  configured OpenTelemetry exporter path.
- Terminal mirroring means tracing fmt/log lines written to stdout or stderr.
- Intentional output means command output written by the command itself, not by
  tracing.

| Command/mode | Class | OTEL configured | OTEL absent | `RUST_LOG` behavior | Terminal mirroring | Intentional terminal output |
| --- | --- | --- | --- | --- | --- | --- |
| `quota status` default/table human | human UI | export telemetry | drop tracing logs | must not enable terminal mirroring | none | one human status block |
| `quota status --format plain` | machine/plain text | export telemetry | drop tracing logs | must not enable terminal mirroring | none | deterministic plain text |
| `quota status --format json` | machine/json | export telemetry | drop tracing logs | must not enable terminal mirroring | none | JSON only |
| `sessions` interactive | human UI | export telemetry | drop tracing logs | must not enable terminal mirroring | none | iocraft picker or concise command error |
| `sessions --list --format table` | human/list text | export telemetry | drop tracing logs | must not enable terminal mirroring | none | TTY human rows or non-TTY plain list |
| `sessions --list --format json` | machine/json | export telemetry | drop tracing logs | must not enable terminal mirroring | none | JSON only |
| `serve` | service runtime | export telemetry | no structured export | may filter OTEL/tracing data but must not add default fmt mirroring | no tracing fmt by default | `listening: <addr>` plus explicit operator errors from the serve loop |

Implementation constraints:
- Command parsing must happen early enough to select the class before installing
  a tracing subscriber.
- OTEL exporter installation is independent of terminal mirroring.
- `RUST_LOG=info` alone is not an operator opt-in for terminal logs in this
  slice.
- This slice does not add a new CLI debug log sink. A future explicit debug
  terminal mirror would need a separate reviewed flag/contract.
- `serve` may keep current explicit lifecycle/error writes that are not tracing
  fmt output, but tests must distinguish those lines from structured logs.

## Vertical Slices

### S1. Presentation Substrate and Dependency Cutover

Behavior:
- Add `iocraft` as the human terminal UI dependency.
- Create a CLI-only presentation boundary with shared width/truncation helpers.
- Replace the old sessions dependency contract with the new `iocraft` contract.
- Preserve machine output and domain/report models.
- Keep `QuotaStatusReport` as the quota UI handoff contract; do not move
  account-selection, quota math, or telemetry metric emission into the
  presentation layer.

Likely files:
- `Cargo.toml`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-cli/src/lib.rs`
- new `crates/codex-router-cli/src/presentation/*` or equivalent

Checkpoint gate:
- `iocraft` compiles in CLI crate.
- New dependency contract fails before adding `iocraft`, passes after cutover.
- No direct `ratatui` dependency.
- Terminal UI imports and dependencies remain in the CLI presentation layer; no
  domain, quota math, auth, websocket, or SQL persistence module imports
  `iocraft`.

Proof:
- targeted manifest/dependency tests.
- `cargo check -p codex-router-cli`.

Split/replan trigger:
- If `iocraft` forces a runtime or edition conflict, stop and revise the plan
  before touching quota/session behavior.

### S2. Quota Status Human Rows and Refresh Behavior

Behavior:
- Replace `write_quota_table` with the account-row human renderer.
- Keep `write_quota_json` machine contract.
- Keep deterministic non-TTY/plain output.
- Remove the second selector summary table from human output.
- Change auto-refresh human behavior so TTY updates one status block and non-TTY
  prints one final status block.
- Add human banner build identity without changing JSON `app_version`,
  telemetry `service.version`, or `--version`.

Likely files:
- `crates/codex-router-cli/src/quota.rs`
- presentation modules
- `crates/codex-router-cli/src/lib.rs` tests as needed

Checkpoint gate:
- golden tests show wide/medium/narrow account rows.
- duplicate-table regression test fails on current behavior, passes after S2.
- JSON tests prove machine contract unchanged.

Proof:
- targeted quota tests.
- `cargo test -p codex-router-cli quota`.

Split/replan trigger:
- If quota report data lacks a field required by the UI, add presentation-only
  derivation if possible; stop before changing quota/account-selection math.

### S3. iocraft Session Picker with Filter Switching

Behavior:
- Replace `InquireSessionsPicker` with an `iocraft` picker.
- Keep public command `codex-router sessions`.
- Show root/provider/source filters and allow switching them in-picker.
- Refilter/reload visible sessions after filter changes.
- Preserve existing startup flags as initial filter state.
- Preserve `--list`, `--format json`, `--last`, and `--dry-run`.
- Own `sessions --list --format table` output-mode behavior:
  - TTY uses the new human row/list presentation.
  - non-TTY prints deterministic plain text with no ANSI.
  - stdout/stderr remain quiet except intentional output/errors.
- Add non-TTY interactive failure.
- Add width behavior for wide, narrow, ultra-narrow, and terminal-too-narrow.
- Change the picker boundary from rendered labels to a richer model/controller
  that can hold `SessionRecord` detail, search text, selected row, and mutable
  root/provider/source filters.

Likely files:
- `crates/codex-router-cli/src/sessions.rs`
- presentation/session picker modules
- `crates/codex-router-cli/src/lib.rs` tests

Checkpoint gate:
- mock-terminal tests cover search, up/down, enter, escape.
- mock-terminal tests cover root, provider, and source filter switching as
  separate assertions.
- mock-terminal width tests cover:
  - wide layout with details pane.
  - narrow layout with one-line filter controls.
  - ultra-narrow layout with stacked abbreviated filter controls.
  - middle truncation for long provider ids, paths, and ids.
  - terminal-too-narrow exit before broken rendering.
- existing session filtering JSON tests still pass.
- `sessions --list --format table` TTY/non-TTY tests pass.
- non-TTY interactive error is concise and does not leak logs.
- fake picker/runner tests may remain as process-launch wiring proof, but they
  do not satisfy the end-state interaction proof.

Proof:
- targeted sessions tests.
- interaction tests using `iocraft::ElementExt::mock_terminal_render_loop`.

Split/replan trigger:
- If provider-id choices cannot be safely inferred from loaded records, plan a
  tiny provider-choice source contract before implementation continues.
- If `iocraft` render-loop ownership fights existing sync `SessionsPicker`
  trait, split into model/controller tests first, then terminal wrapper.

### S4. OTEL-first Logging and Quiet UI

Behavior:
- Classify command mode before telemetry initialization.
- CLI UI commands:
  - export to OTEL when configured
  - drop logs when OTEL is absent
  - never mirror logs to stdout/stderr UI streams
- Machine output commands:
  - never mix logs into stdout/stderr JSON/plain output
- Server runtime:
  - prefer OTEL for structured logs
  - keep only intentional lifecycle/debug terminal output
- Implement the telemetry policy matrix above exactly; do not infer additional
  command classes during implementation without returning to plan review.

Likely files:
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/telemetry.rs`
- tests in `lib.rs` and/or `telemetry.rs`

Checkpoint gate:
- tests prove UI commands do not emit process-start/SQL logs to stderr.
- tests prove OTEL setup path still installs exporter when configured.
- tests prove `RUST_LOG=info` does not re-enable terminal mirroring for human UI
  or machine-output CLI commands.
- tests prove `serve` terminal output policy is intentional and separate from UI.

Proof:
- targeted telemetry tests.
- smoke `quota status`/`sessions --list` outputs with stderr empty except
  intentional command errors.

Split/replan trigger:
- If telemetry must be initialized before parsing for a hard technical reason,
  stop and revise the control-boundary design rather than keeping stderr logs.

### S5. Runtime Smoke on Non-production Port

Behavior:
- Run smoke/e2e validation without touching port `8787`.
- Use an explicit debug port chosen by a helper that binds `127.0.0.1:0`,
  records the assigned port, releases it, then starts the router with
  `--listen-host 127.0.0.1 --port <debug_port>`.
- Reject the smoke run immediately if `<debug_port> == 8787`.
- Capture a transcript at
  `tmp/smoke/2026-06-29-cli-dx-iocraft/non-prod-router-smoke.md` showing every
  runtime command and the selected port.
- Do not kill or restart the user's production router.

Likely files:
- no code change expected, unless test harness helper is needed
- possible repo-local smoke script only if existing test structure supports it

Checkpoint gate:
- all runtime commands in evidence use explicit `--listen-host 127.0.0.1
  --port <debug_port>` or target `http://127.0.0.1:<debug_port>/...`, with
  `<debug_port> != 8787`.
- transcript proves `quota status` and `sessions` surfaces remain quiet/readable.
- serve smoke proves server logging policy without prod port.

Proof:
- smoke transcript.
- any added smoke helper test.

Split/replan trigger:
- If a smoke path requires installed Codex or live auth state, make that proof
  manual/optional and keep automated proof to local deterministic behavior.

## Execution DAG

```text
gate 0: validate source artifacts and repo state
  |
  +-- S1: presentation substrate + dependency contract
        |
        +-- S2: quota rows + refresh behavior
        |
        +-- S4: OTEL-first logging + quiet UI
              |
              +-- S3: session picker + filters
                    |
integration gate: parent reviews diffs for boundary drift and formatting
              |
targeted validation gate:
  cargo check -p codex-router-cli
  cargo test -p codex-router-cli <targeted quota/session/telemetry tests>
              |
S5: runtime smoke on explicit non-8787 debug port
              |
full relevant validation gate:
  cargo test -p codex-router-cli
  cargo nextest run --workspace
  cargo deny check
  cargo audit
              |
implementation-review-swarm
              |
implementation-pr-wrapup, PR-ready not merged
```

Parallelism:
- S2 and S4 can proceed after S1 because they write mostly separate surfaces.
- S3 may start model work after S1, but interactive terminal proof and smoke
  must integrate S4 first because logging affects the picker.
- If one agent implements S2 and another implements S3, keep shared helpers in
  the S1-owned presentation module and avoid independent helper forks.

## Validation Gates

Gate 0:
- `git status --short`
- confirm source spec, review report, and goal state files exist
- confirm no planned runtime command or transcript command uses port `8787`

Slice gates:
- S1: `cargo check -p codex-router-cli`, dependency contract tests
- S2: targeted quota render/refresh/json tests
- S3: targeted session list/picker/filter/non-TTY tests
- S4: targeted telemetry/log-policy tests
- S5: saved smoke transcript at
  `tmp/smoke/2026-06-29-cli-dx-iocraft/non-prod-router-smoke.md` on debug port
  `!= 8787`

Final gates:
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p codex-router-cli`
- `cargo nextest run --workspace`
- `cargo deny check`
- `cargo audit`
- implementation review
- PR-ready wrapup

If an unrelated pre-existing workspace failure blocks a final gate, stop code
edits, report scoped pass/fail evidence, and ask before changing unrelated
infrastructure.

## TDD Notes

- Start S2 with failing golden tests that assert no 11-column headers and no
  second summary table.
- Start S3 with failing mock-terminal tests for filter display and switching.
- Start S3 list-mode work with failing assertions for
  `sessions --list --format table` TTY rows, non-TTY plain output, and empty
  stderr.
- Start S3 width work with failing assertions for the five explicit width
  buckets named in the checkpoint gate.
- Start S4 with failing tests showing stderr fmt logs are currently installed
  for UI commands, machine output commands, and `RUST_LOG=info`.
- Add smoke proof only after lower layers pass.

## Security and Reliability Notes

- Terminal output is a disclosure surface: never render tokens, raw auth files,
  or opaque account ids when labels/hashes are enough.
- JSON output remains machine-only and log-free.
- Session ids from state remain validated before launch.
- Provider ids and paths must truncate safely and not wrap into broken output.
- Serve smoke must not disturb a user's running router on `8787`.

## Open Questions

- Exact keybindings for filter switching can be chosen during implementation,
  but must be visible in the picker footer and covered by mock-terminal tests.
- Provider-id filter choices should come from current command input plus loaded
  session records unless implementation discovers a better existing provider
  source. If this is not enough, stop before inventing a new provider registry.

## Recommended Next Workflow

`shravan-dev-workflow:implementation-execute-plan`
