# Codex Router CLI DX and iocraft Spec

Date: 2026-06-29

Status: draft for review

## Intent

Make codex-router's human CLI screens understandable at a glance.

The CLI exists to explain and control account routing. The default terminal
views should show the selected account, why it was selected, what each account's
quota/activity state is, and how to resume a session without log noise or
wide-table sprawl.

## Non-goals

- Do not change account-selection math in this slice.
- Do not change quota refresh, auth, OAuth, websocket, or SQL data ownership.
- Do not make `quota status` a fullscreen dashboard.
- Do not copy the prodex visual design.
- Do not route normal human output through logs.
- Do not use the production router port `8787` for runtime tests.

## Current Problems

1. `quota status` renders an 11-column table, then a second route summary table.
   Auto-refresh prints cached output, refresh progress, then another full table.

2. The session picker uses `inquire::Select` over one multi-line choice. It
   cannot show a clean list plus selected details pane, and logs can corrupt the
   interactive screen.

3. CLI logs currently go to stderr by default through the tracing fmt layer.
   That makes SQL warnings and process logs visible inside human UI flows.

4. The status screen prints `codex-router 0.1.0` because the workspace package
   version is still `0.1.0`. The UI needs a reliable build identity.

## Source Anchors

- `crates/codex-router-cli/src/quota.rs:1206`: auto-refresh renders twice.
- `crates/codex-router-cli/src/quota.rs:1493`: 11-column quota table.
- `crates/codex-router-cli/src/quota.rs:1567`: second selector summary table.
- `crates/codex-router-cli/src/sessions.rs:232`: session list table.
- `crates/codex-router-cli/src/sessions.rs:390`: `inquire::Select` picker.
- `crates/codex-router-cli/src/telemetry.rs:45`: stderr fmt layer.
- `Cargo.toml:15`: workspace package version.
- `tmp/research-workflows/2026-06-29-iocraft-cli-dx/research-ledger.md`: iocraft fit evidence.

## UI Decision

Adopt `iocraft` for human TTY presentation in `codex-router-cli`.

Use it narrowly:

- `quota status`: one-shot TTY-aware render, not fullscreen.
- `sessions`: interactive render loop with key handling, filtering, and
  selected details.
- non-TTY output: deterministic plain output, no ANSI dependency on terminal
  state.
- JSON output: unchanged, machine-readable, no `iocraft`.

Rationale:

- `iocraft` supports static `write_to_is_terminal` output for width-aware TTY
  rendering and plain fallback for non-TTY writers.
- `iocraft` supports render-loop interaction, terminal events, text input,
  scrolling, terminal-size hooks, and mock-terminal tests.
- A single human presentation boundary is cleaner than stretching `comfy-table`
  for quota and `inquire` for a richer picker.

Cost:

- Adds a component/hook model to the CLI crate.
- Adds transitive terminal UI dependencies, including `crossterm`.
- Requires focused UI golden tests so layout drift is caught.

Compatibility:

- Keep existing `quota status --format table|plain|json` accepted.
- `--format table` is the human TTY/default format, but after this cutover it
  renders account rows instead of the legacy 11-column table.
- `--format plain` remains deterministic non-ANSI text for scripts and pipes.
- `--format json` remains the stable machine contract.
- Remove the legacy wide-table renderer after cutover; do not keep two human
  renderers alive.
- Keep the public interactive session command as `codex-router sessions`.
  This spec does not add or rename it to `sessions resume`.

Dependency contract:

- The existing sessions V1 dependency test that requires `inquire` and rejects
  direct TUI crates must be replaced with a new contract: the CLI may depend on
  `iocraft`, must not directly depend on `ratatui`, and must keep terminal UI
  dependencies inside the CLI presentation layer.

## Presentation Boundary

Create a CLI-only presentation layer. It owns terminal layout, width handling,
colors, keyboard interaction, and output-mode fallback.

It must not own:

- quota calculations
- account selection
- SQL persistence
- auth/OAuth state
- websocket behavior
- telemetry event definitions

Required boundary shape:

- domain/report builders produce plain data models.
- presentation modules convert those models into human TTY/plain UI.
- JSON output continues to serialize report data directly.
- tests can render the same presentation model at fixed widths.
- interactive components are testable through `iocraft` mock-terminal rendering,
  not by snapshotting live terminal state.

## Quota Status Contract

Default human output is account rows, not a wide table.

Required information:

- app identity: version and build identity.
- refresh state: last refresh age and failed refresh summary when present.
- route summary: route band, selected/next account, and concise reason.
- account label and routing state.
- 5h quota percent and reset time.
- weekly quota percent and reset time.
- active sessions per account.
- burn/risk summary in human terms.
- reset credits when useful.

Default shape:

```text
codex-router 0.1.1 (abc1234)    refreshed 5s ago

responses -> askluna
why: weekly healthier; limiting window weekly 100% left

askluna   preferred
  quota      5h 100% left, resets 4h14m     weekly 100% left, resets 6d23h
  activity   0 sessions                     burn 5h 0%/h, weekly 0%/h
  note       selected for next response

matches   available
  quota      5h 100% left, resets 3h20m     weekly 100% left, resets 6d22h
  activity   0 sessions                     burn history insufficient
  note       available if preferred account changes

ssdev     reserve
  quota      5h 78% left, resets 3h16m      weekly 97% left, resets 6d22h
  activity   0 sessions                     burn 5h 12.41%/h, weekly 1.77%/h
  note       protected reserve
```

Narrow shape:

```text
codex-router 0.1.1 (abc1234)    refreshed 5s ago
responses -> askluna    weekly healthier

askluna   preferred   5h 100%   wk 100%   0 sessions
matches   available   5h 100%   wk 100%   0 sessions
ssdev     reserve     5h 78%    wk 97%    0 sessions
```

Refresh behavior:

- A refresh must update the same human status block when attached to a TTY.
- It must not append `updated quota:` plus a second full table.
- Non-TTY output may print a single final status after refresh completes.
- Refresh failures show one concise status line and do not dump logs into the UI.

## Session Picker Contract

The resume picker must use a list/detail model.

Required information:

- screen title: `Resume Codex session`
- scope/filter summary: repo/global, provider, source, result count.
- visible filter controls for the three existing filter dimensions:
  - root: `cwd`, `checkout`, `repo`, `any`
  - provider: `any`, `current`, or a provider id
  - source: `interactive`, `all`, `subagents`
- keyboard control to switch those filters without leaving the picker.
- searchable session list.
- title/preview, age, branch, cwd/project, provider/model, and short id.
- selected-session details pane when width permits.
- deterministic cancel/no-selection behavior.

Default shape:

```text
Resume Codex session

scope: repo    provider: current    source: interactive    4 sessions
filters: [root: repo] [provider: current] [source: interactive]
type to search, tab to filters

sessions
> fixes for async and tokio                         7s ago   main
  we lost the ability to resume a session for this  3d ago   main
  refactoring and improvements                      4d ago   feature/initial...
  questions                                         4d ago   main

selected
  title     fixes for async and tokio
  cwd       .../open-source/ai-dev/codex-router
  branch    main
  provider  codex-router
  model     gpt-5-codex
  id        019ef96...
  preview   fix async runtime and tokio boundaries

enter resume    / search    esc cancel
```

Filter behavior:

- Initial filter state comes from the existing command flags.
- Switching a filter reloads/re-filters the visible session list.
- The picker must make the active filter dimension visible at all widths.
- On narrow terminals, filter controls collapse to one line:

```text
root repo  provider current  source interactive
```

- On ultra-narrow terminals, filter controls stack and abbreviate labels while
  preserving all three dimensions:

```text
r repo
p current
s interactive
```

- Below the minimum width needed to render one readable list row, the picker
  exits with a concise terminal-too-narrow error instead of rendering broken
  controls.
- Provider ids may be longer than the terminal width; truncate them from the
  middle, not the end.
- Filter switching must not change the current public command syntax.

Small-width shape:

- keep the list first.
- hide the selected details pane.
- truncate from the middle for paths and ids.
- never let a row wrap into an unreadable multi-line blob unless it is the
  explicit selected preview.
- all rows and controls must fit the measured terminal width.

## Logs and Telemetry Contract

CLI UI streams must be quiet by default. CLI logs go to OTEL when OTEL is
configured and must not be mirrored to stdout/stderr UI streams.

Required behavior:

- stdout/stderr for UI commands contain only intentional UI output.
- tracing fmt logs are disabled by default for human UI commands before command
  rendering begins.
- OTEL export remains supported and is the preferred destination for CLI logs.
- `RUST_LOG` alone must not re-enable terminal mirroring for CLI UI commands.
- SQL slow-query logs must not appear inside the session picker.
- If OTEL is not configured for CLI UI commands, logs are dropped for this
  slice; they are not printed into the UI and no new alternate debug sink is
  added by this spec.

Server behavior:

- `codex-router serve` should prefer OTEL for structured logs.
- Server terminal output should be limited to intentional operator lifecycle
  lines or explicit debug terminal mirroring.
- The implementation plan must define the exact server log policy before code
  changes touch telemetry initialization.

Control boundary:

- CLI dispatch classifies commands as human UI, machine output, or service
  runtime before initializing terminal log mirroring.
- Human UI commands include `quota status` in human formats and interactive
  `sessions`.
- Machine output commands include JSON modes and should never receive fmt logs
  on stdout/stderr.
- Service runtime commands such as `serve` may keep concise terminal lifecycle
  messages, but structured logs prefer OTEL.
- OTEL export is independent of terminal mirroring: quiet UI must still export
  telemetry when OTEL is configured.

## Privacy and Security Contract

This spec does not add new privileged actions, network requests, auth flows, or
state mutation. The security-sensitive risk is terminal disclosure.

Required behavior:

- Do not render access tokens, refresh tokens, cookie values, or raw auth files.
- Do not render full opaque account ids when a label or hash is sufficient.
- Do not mix logs into JSON or interactive UI output.
- Do not let debug terminal logging become enabled by default.

## Version Contract

Every human status screen must display useful build identity.

Required behavior:

- show the package version from Cargo.
- include git revision or build metadata when available on human banners.
- update the workspace version during release/install workflows.
- add a test that prevents a hard-coded stale version string.
- keep JSON `app_version`, telemetry `service.version`, and `--version`
  version-only unless a separate machine contract change explicitly expands
  them.

## Output Modes

Command and mode matrix:

```text
command / mode              TTY behavior                         non-TTY behavior
quota status default        human account rows                    plain deterministic text
quota status --format table human account rows                    plain deterministic text
quota status --format plain plain deterministic text              plain deterministic text
quota status --format json  JSON only                             JSON only
sessions interactive        iocraft picker                        fail with concise non-TTY error
sessions --list table       human list rows                       plain deterministic text
sessions --list json        JSON only                             JSON only
```

Human TTY:

- use `iocraft` rendering.
- allow color only when terminal/color policy permits; `NO_COLOR` and non-TTY
  output disable color.
- respect terminal width.

Human non-TTY:

- no ANSI control sequences.
- no interactive render loop.
- print deterministic text suitable for pipes and snapshots.

JSON:

- unchanged machine contract.
- no decorative formatting.
- no logs mixed into JSON.

## Proof Expectations

Unit:

- quota presentation model maps route/account states into row labels.
- version/build identity formatter handles version-only and version+revision.
- log policy chooses quiet UI output by default.

Golden rendering:

- `quota status` at wide width, medium width, and narrow width.
- refresh success does not duplicate the full status block.
- refresh failure is concise.
- non-TTY quota output has no ANSI.

Interaction:

- session picker filters by typed input.
- session picker shows and switches root/provider/source filters.
- up/down changes selection.
- enter returns the selected session id.
- escape cancels cleanly.
- selected detail pane hides at narrow width.
- session-picker interaction tests use `iocraft` mock-terminal rendering with
  simulated key events.

Smoke:

- `codex-router quota status` shows one readable status screen.
- `codex-router sessions` opens without SQL/log noise on screen.
- quiet CLI output and OTEL export have separate proof cases.
- any retained server terminal lifecycle/debug output has separate proof from
  CLI UI quietness.
- every runtime proof path that starts or targets a router uses an explicit
  debug port that is not `8787`.

## Review Readiness

This spec is ready for spec review when reviewers can answer:

- Does the spec preserve account-routing clarity as the main product purpose?
- Are logs, version identity, and non-TTY behavior specified enough to avoid
  another partial UI-only fix?
- Is the `iocraft` adoption boundary narrow enough to implement without changing
  quota math or runtime routing?
