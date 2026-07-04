# Quota Status Reset-Pace UX Implementation Plan

Date: 2026-07-04
Repo: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router`
Branch observed: `fix/quota-live-layout-selection`
Plan status: reviewed and ready for implementation, not executed
Review sources:

- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-revised-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-third-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-fourth-plan-review.md`
- `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-final-plan-review.md`

## Source Coverage

Source is the accepted chat design from this session plus the accepted
plan-review findings. No source spec file exists.

Accepted product requirements:

- `quota status` and `sessions` remain read-only observers; `serve` owns DB writes.
- Quota status keeps showing last-known quota values even when data is stale or aging.
- 15 minutes is fresh enough for the `quota status` sample-confidence/display path.
- 15 minutes is not, by this plan, a new runtime selector-authority window.
- Runtime routing freshness remains governed by the existing selector stale-after authority unless a separate routing spec explicitly changes it.
- Stale is a confidence/status annotation, not a reason for the status UI to drop values.
- Weekly quota display as a bar plus percent remains.
- Remove repeated `needs refresh`, `safe pace unknown`, `ahead to reset`, and old safe-pace prose from human-facing table/plain/TUI status.
- Preserve machine-readable JSON enum meanings unless a new field is explicitly named in this plan.
- Replace safe-pace language with reset-pace language in human-facing output.
- Burn balance chart is centered at `1.0x reset pace`.
- Healthy band is `0.8x..=1.2x`, green.
- Below `0.8x` is yellow under-burning.
- Above `1.2x` is red over-burning.
- Fill grows outward from the center marker, not left-to-right like a progress bar.
- The center marker takes the active state color when ANSI styling is emitted.
- If burn multiple cannot be computed, show `burn unavailable` with sample confidence, not a fake empty meter.

Review findings incorporated:

- Split status/sample freshness from runtime selector authority.
- Classify the current dirty product diffs before overlapping implementation edits.
- Define a typed reset-pace burn view-model contract before presentation work.
- Resolve human/plain/JSON output contracts before coding.
- Tie stale-value rendering to no-provider-I/O proof in one durable fixture.
- Add direct proof that runtime selector authority remains at 300 seconds if status display uses 15 minutes.
- Expand visual captures and manual acceptance criteria.
- Add explicit CI-equivalent validation gates.
- Decide account-label safety and prove it.
- Add degraded-read proof that stale values display without preferred routing authority.
- Specify telemetry sample-age bucket and forbidden-label contracts across status telemetry surfaces.
- Route post-implementation proof to `implementation-review-swarm`, not another plan review.
- Route runtime-authority proof through state/projection stale-after behavior, not selection-only stale facts.
- Define status sample age from displayed value-bearing quota windows, not refresh status or selector stale status.
- Add a serialized shared status DTO/view-model contract step before integrating sample confidence and reset-pace fields.
- Define reset-pace module ownership before coding the renderer.
- Add workflow lint to the CI-equivalent proof gate.
- Name visual capture cases so under/healthy/over/unavailable reset-pace states are all reviewed.
- Add adversarial renderer/source-guard proof that typed reset-pace fields drive rendering without renderer-side string parsing.
- Expand visual capture requirements into a `case x width x style` artifact matrix with paired plain and ANSI artifacts for each named state.
- Expand telemetry proof so status telemetry covers tracing/log attributes as well as OTel metrics.

Current repo evidence inspected:

- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/presentation/quota.rs`
- `crates/codex-router-state/src/selection_projection.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-cli/src/sessions.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-selection/src/burn_down.rs`
- `.github/workflows/ci.yml`

## Key Decision

The 15-minute rule is status-display freshness only.

This plan does not change persisted selector stale-after authority, runtime
route selection, `serve` selector reads, `sqlite.rs` stale marking, or
`codex-router-proxy` routing behavior. Existing selector authority remains
300 seconds unless a separate routing-focused spec changes it.

The status surface may display a value-bearing sample as:

```text
fresh:  age <= 15m
stale:  age > 15m
unknown: no sample age or no value-bearing sample
```

`sample_age_seconds` is derived from displayed value-bearing quota windows. It
is not derived from refresh status, selector stale-after timestamps, selector
window status, or runtime routing authority. When a row displays multiple
value-bearing windows, sample age uses the conservative oldest
`observed_unix_seconds` among those displayed windows. `unknown` means no
displayed value-bearing window has an observed sample.

There is no separate `aging` bucket in this revision. The user-visible distinction
is intentionally simple: values can be stale and still shown.

## Goal

Make `codex-router quota status` readable as a quota health and reset-pace surface:

- It should answer which account is selected and why.
- It should show weekly and 5h quota values even when samples are stale.
- It should make sample freshness visible once, not repeat refresh warnings.
- It should show burn balance as a centered reset-pace meter.
- It should preserve observer-only DB behavior and no-provider-I/O status semantics.
- It should not silently change `serve` routing authority.

## Non-Goals

- Do not add provider I/O to `quota status`.
- Do not make `quota status` or `sessions` write to router or Codex DBs.
- Do not change runtime selector freshness, route weights, or proxy account selection.
- Do not change auth, credential storage, or provider refresh protocol.
- Do not introduce live OAuth/provider proof as a required validation gate.
- Do not rely on color alone; non-ANSI text must still carry meaning.
- Do not perform dependency or terminal-width refactors beyond preserving the current branch baseline.

## Gate 0: Re-Anchor Worktree and Baseline

Before any implementation edit:

1. Run `git status --short --branch`.
2. Inspect the current diff for:
   - `Cargo.toml`
   - `Cargo.lock`
   - `crates/codex-router-cli/Cargo.toml`
   - `crates/codex-router-cli/src/presentation/quota.rs`
3. Classify the existing `terminal_size` to `crossterm` terminal-width diff as:
   - accepted branch baseline to preserve, or
   - unrelated blocker that must be resolved before editing overlapping files.
4. Do not revert user or pre-existing work.
5. If the dependency diff remains in the final branch, include dependency policy proof through `cargo deny check` and `cargo audit`.

Split/replan trigger:

- If the current dirty product diff is unrelated and cannot be preserved cleanly, stop before editing overlapping files.

## Output Contract

Human-facing table/plain/TUI output:

- `needs refresh` must not be repeated as row/window filler.
- `safe pace`, `safe pace unknown`, and `ahead to reset` must not appear.
- Quota windows show known values even when sample confidence is stale.
- Stale value-bearing samples use compact metadata:
  - `sample fresh <age>`
  - `sample stale <age>`
  - `sample unknown`
- Unknown value cases use `burn unavailable`, not an empty meter.
- Weekly value remains compact: `weekly <bar> <percent>`.
- Non-ANSI output includes semantic labels: `under`, `healthy`, `over`, or `burn unavailable`.

Machine-readable JSON:

- Existing enum meanings are preserved unless a new field is explicitly added.
- Banned human phrases are not blanket-banned from existing JSON enum fields when they are stable machine contract.
- If sample confidence is exposed to JSON, add explicit fields rather than overloading display strings:
  - `sample_confidence`: `fresh | stale | unknown`
  - `sample_age_seconds`: optional number
- JSON must keep account IDs hashed and must not expose provider secrets, raw provider errors, or unsafe raw labels.

Account labels:

- Quota status output must use `safe_account_label(account.label(), account.account_id())` or an equivalent single sanitized display label for table/plain/TUI/JSON.
- Local simple labels remain visible when the existing helper classifies them as safe.
- Unsafe labels such as emails, bearer-like tokens, and raw account IDs render as deterministic safe tags.

Telemetry:

- Telemetry means both OTel metrics and tracing/log attributes emitted from the
  status surface.
- Do not add exact sample ages or raw sample strings to telemetry.
- If telemetry gains sample confidence, use low-cardinality buckets only: `fresh | stale | unknown`.
- Forbidden telemetry labels include `sample.age_seconds`, `sample.age_text`, `provider.error`, `account.label`, and raw account IDs.
- Forbidden telemetry values include exact sample ages/text, raw provider
  errors, raw account IDs, and unsafe raw account labels.

## Typed Reset-Pace View Model Contract

Implementation must define a typed reset-pace model before presentation work.
The exact Rust shape may follow local conventions, but it must carry these
semantics without requiring presentation code to parse display strings:

```text
ResetPaceState:
  under_burning
  healthy
  over_burning
  unavailable

ResetPaceViewModel:
  state
  multiple_label       # e.g. "0.79x reset pace"
  semantic_label       # under | healthy | over | burn unavailable
  meter_left_segments
  meter_right_segments
  center_marker
  unavailable_reason
```

Ownership:

- DTO/view-model types live in `crates/codex-router-cli/src/presentation/quota.rs`
  alongside the existing quota status view-model structs, or in a named
  presentation-facing module introduced only if it reduces coupling.
- `crates/codex-router-cli/src/quota.rs` owns sample-confidence derivation,
  reset-pace math, classification, and construction of the typed
  presentation-facing values.
- Slice 2 owns reset-pace math, classification, and typed model construction.
- Slice 3 owns ANSI/non-ANSI rendering from the typed model.
- Presentation code must not infer state by parsing reset/sample display strings,
  semantic labels, multiple labels, or meter glyph strings. Test code may use
  string assertions to verify final output, but production renderer code must
  branch from typed fields.
- The renderer proof must include an adversarial fixture where typed
  `ResetPaceState`, segment counts, and center marker drive output while labels
  contain sentinel or conflicting strings that would fail if parsed.

## Shared Status DTO Contract

After Gate 0 and before integrating Slice 1 or Slice 2 into the row renderer,
implementation must define the shared status DTO/view-model contract that carries
both sample metadata and reset-pace metadata into presentation.

The contract must carry these semantics without requiring presentation code to
parse display strings:

```text
SampleConfidence:
  fresh
  stale
  unknown

SampleMetadata:
  confidence
  age_label
  age_seconds
  semantic_label     # sample fresh | sample stale | sample unknown

Quota status row/detail DTO:
  displayed quota windows
  sample metadata from displayed value-bearing windows
  reset-pace view model
```

Slice 1 and Slice 2 may develop pure helper tests in parallel only after this
contract is defined. Integration through `QuotaStatusRow`,
`QuotaStatusAccountViewModel`, and `QuotaSelectedAccountViewModel` is a
single-owner or sequential step.

Renderer guard:

- Renderer code must consume typed `SampleMetadata` and `ResetPaceViewModel`
  fields directly.
- Renderer code must not parse `age_label`, `semantic_label`, `multiple_label`,
  or meter glyph strings to rediscover freshness, reset-pace state, segment
  counts, color, or unavailable state.
- The source guard proof must fail if the renderer ignores typed state and
  derives behavior from display labels or glyph text.

## Vertical Slice Cards

### Slice 1: Status-Only Sample Freshness and Stale Values

Source requirements:

- 15 minutes is fresh for `quota status` sample display.
- Runtime selector authority remains unchanged at 300 seconds.
- Stale data still displays in status.
- `needs refresh` should not repeat in every row/window.

Behavior:

- Add a status-only sample confidence classifier with a 900-second display threshold.
- Derive sample age from displayed value-bearing `DisplayQuotaWindow`
  observations. Do not derive it from `QuotaRefreshStatusView`,
  selector stale-after timestamps, selector window status, or runtime routing
  authority.
- If both 5h and weekly value-bearing windows are displayed, use the oldest
  displayed observed sample age for row-level sample metadata unless a later
  plan explicitly chooses per-window metadata.
- Keep existing persisted selector stale-after writes and runtime selector stale marking unchanged.
- Keep last-known 5h and weekly values visible when windows are stale but value-bearing.
- Present sample age/confidence separately from quota value text.
- Existing rows with old 300-second stale-after values still show values and sample age.

Likely touched files:

- `crates/codex-router-cli/src/quota.rs`
- Tests in `crates/codex-router-cli/src/quota.rs`
- Integration tests in `crates/codex-router-cli/src/lib.rs`

Test-only write surfaces for runtime-authority proof:

- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/selection_projection.rs` only if the exact
  state/projection proof cannot live in `sqlite.rs`

Read-only proof anchors:

- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-selection/src/burn_down.rs`

Checkpoint:

- Unit tests prove status sample confidence at 14m59s, 15m00s, and 15m01s.
- Unit tests prove row-level sample age uses the oldest displayed value-bearing
  quota window when 5h and weekly windows have different observed ages.
- A fixture proves fresh refresh status does not make an older displayed quota
  sample fresh.
- A stale-but-value-bearing fixture proves weekly/5h values still render.
- A state/projection routing-authority regression proves persisted selector
  windows are eligible at 299 seconds and stale at 300/301 seconds.

Proof:

- Unit: status sample confidence helper and stale value formatting.
- Integration: `quota_status_preserves_stale_selector_window_values_without_provider_io`.
- State/projection regression: runtime selector authority remains 300 seconds.
- Guard: no provider calls and read-only DB opens remain unchanged.

Split/replan trigger:

- Any implementation that changes persisted selector stale-after writes, `selector_windows_are_stale(...)`, proxy selection, or selection weights is out of this plan and requires a routing spec.

### Slice 2: Reset-Pace Burn Model

Source requirements:

- Replace safe-pace wording with reset-pace wording.
- Chart is centered at `1.0x reset pace`.
- Healthy `0.8x..=1.2x` green, under-burning yellow, over-burning red.
- Fill grows from center outward.
- Unknown burn renders as unavailable, not a low/empty meter.

Behavior:

- Rename/replace human-facing `safe pace` helpers with reset-pace helpers.
- Compute burn multiple from the existing `quota_pace_load` concept, but render as `x.xx reset pace`.
- Build the typed reset-pace view model before presentation.
- Classify:

```text
< 0.8x      under_burning   yellow   under
0.8..1.2x  healthy         green    healthy
> 1.2x     over_burning    red      over
unknown    unavailable     default  burn unavailable
```

- Generate center-origin meter segments from the typed state.

Likely touched files:

- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/presentation/quota.rs`

Checkpoint:

- Pure tests prove 0.79x, 0.80x, 1.00x, 1.20x, and 1.21x classify correctly.
- Tests prove unavailable burn does not produce a fake meter.
- Rendering tests prove center-origin fill and active color class mapping.

Proof:

- Unit: burn multiple, classification, typed model, glyph segment generation.
- Rendering: table/TUI text contains `reset pace`, never `safe pace`.
- Adversarial rendering: typed state/segments/color drive the rendered meter
  even when display labels contain sentinel or conflicting strings.
- Negative copy checks: no `ahead to reset`, `safe pace unknown`, or fake unknown meter in human-facing outputs.

Split/replan trigger:

- If the typed model cannot carry color/center semantics cleanly, stop and revise the plan before presentation work.

### Slice 3: Quota Status Layout and Copy Cleanup

Source requirements:

- Weekly bar plus percent remains.
- Remove refresh-prose clutter from rows.
- Stale/freshness appears once as compact sample metadata.
- Selected account panel remains readable.

Behavior:

- Keep weekly row value compact: `weekly <bar> <percent>`.
- Show sample metadata as `sample fresh <age>`, `sample stale <age>`, or `sample unknown`.
- Replace row/detail `Burn pace` prose with reset-pace chart and compact label.
- In selected details, keep quota windows, burn, activity, and reason visually separate.
- Do not widen columns just to fit repeated prose.
- Preserve the existing terminal-width baseline, including the current `crossterm` width read if Gate 0 accepts it as branch baseline.

Likely touched files:

- `crates/codex-router-cli/src/presentation/quota.rs`
- `crates/codex-router-cli/src/quota.rs`

Checkpoint:

- Width contract passes at existing tested widths.
- Reflow and focus tests still pass.
- Visual capture artifacts are generated for named `fresh-healthy`,
  `stale-under`, `degraded-over`, and `unavailable-burn` cases.

Proof:

- Rendering tests for 48, 72, 90, 120, and 160 columns.
- TUI focus tests with a stale secondary row.
- Ignored visual capture test produces a `case x width x style` matrix:
  `fresh-healthy`, `stale-under`, `degraded-over`, and `unavailable-burn` at
  one narrow width and one sidecar width, with paired non-ANSI `.txt` and ANSI
  `.ansi` artifacts for every case/width pair.
- Manual capture checklist confirms center-origin meter, semantic labels,
  compact sample age, ANSI active marker color, non-ANSI meaning, and no
  repeated banned phrases.

Split/replan trigger:

- If the centered meter plus sample metadata cannot fit at narrow widths, replan narrow layout before coding further.

### Slice 4: Observer, Redaction, and Degraded-Read Guardrails

Source requirements:

- `quota status` and `sessions` never write DBs.
- Status does not perform provider I/O.
- Stale data is visible but not silently promoted into fresh routing authority.
- Redaction and telemetry stay low-cardinality.
- Degraded read state remains visible and non-authoritative.

Behavior:

- Preserve `AsyncSqliteStateStore::open_read_only(...)` in quota status.
- Preserve `read_only(true)`, `create_if_missing(false)`, and `query_only=ON` in sessions.
- Preserve read-only active-client and projection paths.
- Preserve degraded fallback behavior when read-only projection fails.
- In degraded mode, show value-bearing stale quota facts without marking a preferred routing authority.
- Use safe account labels for status output.
- Do not add raw account IDs, tokens, provider errors, exact sample ages, or high-cardinality text to telemetry.
- Treat tracing/log attributes and OTel metric labels as telemetry for this
  guard.

Likely touched files:

- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/presentation/quota.rs`

Read-only proof anchors:

- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/selection_projection.rs`
- `crates/codex-router-cli/src/sessions.rs`

Checkpoint:

- Existing read-only projection purity tests stay green.
- Existing no-provider-I/O status test is updated or supplemented by the stale-value fixture.
- Redaction tests cover unsafe account labels.
- Degraded fixture proves no preferred authority is shown.

Proof:

- Unit/integration: existing read-only store and projection purity tests.
- Integration: `quota_status_preserves_stale_selector_window_values_without_provider_io`.
- Integration: degraded stale-value fixture with `route_result = degraded` and `preferred_next_account_hash = null`.
- Redaction: JSON/human/telemetry tests.
- Telemetry proof must cover status tracing/log attributes and OTel metrics.

Split/replan trigger:

- Any proposed call from status into refresh, writable active-client reads, rollup refresh, migrations, provider I/O, proxy routing, or selector authority changes is a hard stop.

## Requirements / Proof Matrix

| ID | Requirement | Source | Task | Proof modality | Layer | Evidence source | Freshness guard | Red/green |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | 15m is status display freshness only | chat + review | Slice 1 | helper boundary tests | unit | `quota.rs` tests | sample age from displayed windows at 899s/900s/901s | yes |
| R2 | Runtime selector authority remains 300s | review blocker | Slice 1 | state/projection stale-after regression | integration | `sqlite.rs` or exact state/projection equivalent | persisted stale-after eligible at 299s, stale at 300s/301s | yes |
| R3 | Stale/aging data still shows quota values | chat design | Slice 1 | stale fixture assertions | unit + integration | `quota.rs`, `lib.rs` tests | stale row with 5h/weekly values | yes |
| R4 | No provider I/O from stale status display | user invariant + review | Slice 1/4 | stale DB fixture | integration | `quota_status_preserves_stale_selector_window_values_without_provider_io` | same fixture as R3 | yes |
| R5 | No repeated `needs refresh` in human row/window prose | chat design | Slices 1/3 | negative string assertions | unit + rendering | table/plain/TUI output | JSON excluded unless explicitly changed | yes |
| R6 | Preserve JSON enum compatibility unless fields are named | review | Slices 1/3/4 | JSON compatibility assertions | integration | `quota_status_json...` tests | compare existing enum meanings | yes |
| R7 | Replace safe pace with reset pace | chat design | Slice 2 | negative/positive string assertions | unit + rendering | `quota.rs`, `presentation/quota.rs` | unavailable burn prints unavailable | yes |
| R8 | Burn meter is centered at 1.0x reset pace | chat design | Slice 2 | typed model/glyph tests | unit | reset-pace helper tests | threshold set | yes |
| R9 | Healthy 0.8x..=1.2x green; under yellow; over red | chat design | Slice 2/3 | classification and ANSI/non-ANSI rendering checks | unit + rendering | `presentation/quota.rs` | center marker active color | yes |
| R9a | Shared DTO carries sample and reset-pace metadata without string parsing | revised + third review | shared contract | compile + adversarial renderer/source-guard tests | unit + rendering | `quota.rs`, `presentation/quota.rs` | typed fields drive rendering while labels/glyphs contain sentinel conflicts | yes |
| R10 | Weekly bar plus percent remains visible | chat design | Slice 3 | width capture assertions | rendering | `quota_status_width_contract_preserves_layout` | fresh and stale fixtures | yes |
| R11 | Selected detail focus/reflow still works | existing TUI behavior | Slice 3 | mock terminal tests | rendering | `presentation/quota.rs` focus tests | stale secondary row | no, update existing |
| R12 | `quota status` remains DB read-only | user invariant + code | Slice 4 | existing read-only/projection purity tests | unit + integration | state and CLI tests | stale scenario uses read-only path | no unless call chain changes |
| R13 | `sessions` remains DB read-only/query-only | user invariant + code | Slice 4 | existing sessions read-only assertions or code inspection in review | integration/review | `sessions.rs` tests or review anchor | command path still query-only | no unless touched |
| R14 | No secret/raw ID/unsafe label leaks | security review | Slice 4 | redaction fixtures | unit + integration | table/plain/TUI/JSON tests | unsafe label fixtures | yes |
| R15 | Telemetry remains low-cardinality across tracing/log attributes and OTel metrics | security review | Slice 4 | telemetry contract assertions | unit | telemetry label/source guard test | forbid exact sample age/text, raw provider errors, raw account IDs, raw account labels, and unsafe labels in telemetry labels or values | yes, existing proof is metrics-only |
| R16 | Degraded read does not imply preferred authority | security review | Slice 4 | degraded fixture | integration + rendering | JSON/human/TUI assertions | route_result degraded, preferred hash null | yes |
| R17 | Visual review artifacts cover redesigned UI | UX proof | Slice 3 | ignored capture generation + manual checklist | visual/manual | capture artifacts | `fresh-healthy`, `stale-under`, `degraded-over`, `unavailable-burn`; narrow + sidecar; ANSI + non-ANSI | manual |
| R18 | CI-equivalent quality gates pass | review | final gate | command validation | quality + test + dependency + workflow lint | fmt, clippy, nextest, deny, audit, actionlint | current worktree, scoped failures reported | no red, final proof |

## Execution DAG

```text
gate 0: re-anchor worktree and classify existing product diffs
  |
shared contract gate: define sample/reset-pace DTO fields and module ownership
  |
  +-- lane A1: Slice 1 pure sample freshness helpers
  |     scope: quota.rs helper tests
  |     proof: sample age source + 899/900/901s tests
  |
  +-- lane B1: Slice 2 pure reset-pace math/model helpers
  |     scope: quota.rs helper/model tests
  |     proof: pure classification/model/glyph tests
  |
serialized integration gate 1: wire shared DTO through QuotaStatusRow/ViewModel
  |
state/projection guard gate: parent confirms 15m display does not change 300s routing authority
  |
integration gate 2: parent confirms typed reset-pace model feeds view-model without string parsing
  |
  +-- lane C: Slice 3 TUI/table/plain layout
  |     scope: presentation/quota.rs + quota.rs view-model mapping
  |     proof: width/reflow/focus tests and capture artifacts
  |
  +-- lane D: Slice 4 observer/redaction/degraded guardrails
        scope: quota.rs/lib.rs tests and assertions
        proof: read-only, no provider I/O, redaction, degraded tests
  |
integration gate 3: run focused quota/status/presentation/state/proxy guard tests
  |
visual/manual validation gate: inspect generated quota captures
  |
full CI-equivalent validation gate
  |
implementation-review-swarm
  |
implementation-pr-wrapup
```

Parallelization:

- Slice 1 and Slice 2 pure helper tests can start in parallel only after the
  shared DTO contract is defined.
- Integration through `QuotaStatusRow`, `QuotaStatusAccountViewModel`, and
  `QuotaSelectedAccountViewModel` is serial or single-owner because both sample
  metadata and reset-pace metadata share the same row/detail contract.
- Slice 3 depends on the typed view-model contract from Slice 2 and the sample-label contract from Slice 1.
- Slice 4 can run in parallel as guardrail proof, but it must not mutate routing authority or provider paths.

## Task Sequence

1. Gate 0: re-check `git status --short --branch`, inspect existing diffs, and classify the `crossterm` terminal-width diff as accepted baseline or blocker.
2. Define the shared status DTO/view-model contract:
   - sample confidence and sample age fields from displayed value-bearing windows
   - reset-pace typed fields for state, label, segments, center marker, and unavailable reason
   - module ownership between `quota.rs` construction and `presentation/quota.rs` rendering
3. Add red tests for status-only freshness:
   - `899s`, `900s`, `901s` sample confidence.
   - oldest displayed value-bearing window owns row-level sample age.
   - fresh refresh status does not make an older displayed quota sample fresh.
   - stale values still visible.
   - state/projection stale-after proof shows persisted selector windows are eligible at 299 seconds and stale at 300/301 seconds.
4. Implement Slice 1:
   - Add status-only sample confidence.
   - Do not change persisted selector stale-after writes.
   - Do not change `selection_projection.rs`, `sqlite.rs`, proxy routing, or selection weights unless replanned.
   - Only test additions in `sqlite.rs`/`selection_projection.rs` are allowed for R2 proof unless replanned.
5. Add red tests for reset-pace classification and typed model.
6. Implement Slice 2:
   - Rename human-facing concepts away from safe pace.
   - Compute/display `x.xx reset pace`.
   - Build typed reset-pace model and center-origin meter data.
7. Integrate Slice 1 + Slice 2 through the shared DTO/view-model shape as a single-owner or serial step.
8. Add/update TUI/table/plain rendering tests for rows, details, widths, focus, stale samples, non-ANSI meaning, ANSI marker color, and JSON compatibility.
   - Include an adversarial renderer/source-guard test proving typed
     reset-pace/sample fields drive rendering without parsing labels or glyphs.
9. Implement Slice 3 presentation.
10. Add/update Slice 4 guardrail tests:
   - stale values plus no provider I/O in one fixture
   - safe account labels
   - telemetry forbidden labels and values across tracing/log attributes and
     OTel metrics, because existing proof is metrics-only
   - degraded stale-value output without preferred authority
11. Generate paired plain and ANSI visual capture artifacts for every
    `fresh-healthy`, `stale-under`, `degraded-over`, and `unavailable-burn`
    case at narrow and sidecar widths.
12. Run focused validation, manual visual validation, and full CI-equivalent validation.
13. Route to `implementation-review-swarm` after implementation proof.
14. Route to `implementation-pr-wrapup` after implementation review findings are addressed or explicitly rejected.

## Write Surfaces

Allowed likely write surfaces:

- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-cli/src/presentation/quota.rs`
- Tests colocated in the files above
- CLI integration tests in `crates/codex-router-cli/src/lib.rs`

Allowed test-only write surfaces for runtime-authority proof:

- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/selection_projection.rs` only if the
  299/300/301 persisted stale-after proof cannot live in `sqlite.rs`
- `crates/codex-router-proxy/src/account_selection.rs` only for an optional
  proxy-level proof that runtime selection consumes stale-marked projected
  windows

Conditionally allowed only to preserve or validate the current branch baseline:

- `Cargo.toml`
- `Cargo.lock`
- `crates/codex-router-cli/Cargo.toml`

Read-only production proof anchors:

Test additions in the allowed test-only list above are the only exception. Do
not change production behavior in these modules under this plan.

- `crates/codex-router-state/src/selection_projection.rs`
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-cli/src/sessions.rs`
- `crates/codex-router-proxy/src/account_selection.rs`
- `crates/codex-router-selection/src/burn_down.rs`

Guarded / avoid unless replanned:

- provider refresh paths in `crates/codex-router-cli/src/quota.rs`
- auth, secret storage, migration, provider HTTP code
- runtime selector freshness, proxy routing, selection policy, or DB stale marking

## Validation Gates

Focused gates for implementation:

```text
cargo test -p codex-router-cli quota::tests::quota_status_sample_confidence_uses_15_minute_display_boundary -- --exact
cargo test -p codex-router-cli quota::tests::quota_status_sample_confidence_uses_displayed_value_window_age -- --exact
cargo test -p codex-router-cli quota::tests::quota_status_reset_pace_classifies_thresholds -- --exact
cargo test -p codex-router-cli quota::tests::quota_status_reset_pace_unavailable_has_no_fake_meter -- --exact
cargo test -p codex-router-cli quota::tests::quota_status_shared_dto_carries_sample_and_reset_pace_without_string_parsing -- --exact
cargo test -p codex-router-cli quota::tests::quota_status_table_separates_quota_bars_from_burn_bars -- --exact
cargo test -p codex-router-cli quota::tests::quota_status_width_contract_preserves_layout -- --exact
cargo test -p codex-router-cli presentation::quota::tests::quota_status_renderer_uses_reset_pace_fields_without_parsing_strings -- --exact
cargo test -p codex-router-cli presentation::quota::tests::quota_status_uses_sidecar_only_at_160_columns -- --exact
cargo test -p codex-router-cli presentation::quota::tests::quota_status_reflows_when_terminal_width_changes -- --exact
cargo test -p codex-router-state selection_projection::tests::read_only_projection_does_not_call_refresh_rollups_or_mutating_active_count_reader -- --exact
```

New or updated integration gates:

```text
cargo test -p codex-router-cli quota_status_preserves_stale_selector_window_values_without_provider_io -- --exact
cargo test -p codex-router-cli quota_status_degraded_stale_values_do_not_expose_preferred_authority -- --exact
cargo test -p codex-router-cli quota_status_redacts_unsafe_account_labels -- --exact
cargo test -p codex-router-cli quota_status_json_preserves_machine_contract_for_refresh_states -- --exact
cargo test -p codex-router-cli quota_status_telemetry_contract_uses_scrubbed_low_cardinality_labels -- --exact
```

The telemetry contract test may be updated or paired with an equivalent
source/fixture guard, but it must cover status tracing/log attributes as well
as OTel metric labels and values.

Runtime-authority guard gate:

```text
cargo test -p codex-router-state sqlite::tests::selector_inputs_mark_windows_stale_at_persisted_300_second_boundary -- --exact
```

If the exact runtime-authority guard belongs in another existing test module, the executor may use the repo-local equivalent, but it must prove the same claim at the persisted state/projection boundary: status display freshness does not extend selector authority, and persisted selector windows are eligible at 299 seconds and stale at 300/301 seconds.

Optional proxy-level runtime-consumption guard:

```text
cargo test -p codex-router-proxy account_selection::tests::quota_selector_uses_stale_marked_projected_windows -- --exact
```

Broader relevant gates:

```text
cargo test -p codex-router-cli quota_status
cargo test -p codex-router-cli quota::tests::quota_status_
cargo test -p codex-router-cli presentation::quota::tests::quota_status_
cargo test -p codex-router-state selection_projection::tests::read_only_projection_
```

Visual/manual gate:

```text
cargo test -p codex-router-cli quota::tests::quota_status_capture_artifacts_for_design_review -- --ignored --exact
```

Named visual capture matrix:

- `fresh-healthy-48.txt`
- `fresh-healthy-48.ansi`
- `fresh-healthy-160.txt`
- `fresh-healthy-160.ansi`
- `stale-under-48.txt`
- `stale-under-48.ansi`
- `stale-under-160.txt`
- `stale-under-160.ansi`
- `degraded-over-48.txt`
- `degraded-over-48.ansi`
- `degraded-over-160.txt`
- `degraded-over-160.ansi`
- `unavailable-burn-48.txt`
- `unavailable-burn-48.ansi`
- `unavailable-burn-160.txt`
- `unavailable-burn-160.ansi`

The executor may choose a different existing sidecar width only if the plan is
updated before implementation; the matrix must still include one narrow width,
one sidecar width, and paired plain/ANSI artifacts for every named case.

Manual checklist for generated captures:

- fresh sample shows values and compact `sample fresh <age>`
- stale sample shows values and compact `sample stale <age>`
- degraded read shows values without preferred authority
- under/healthy/over reset-pace states are visible in text
- unknown burn shows `burn unavailable`
- center-origin meter fills from the center marker
- ANSI capture shows the center marker in the active state color
- non-ANSI capture carries `under`, `healthy`, `over`, or `burn unavailable`
- no repeated `needs refresh`, `safe pace`, `safe pace unknown`, or `ahead to reset` in human-facing output
- no narrow-width overlap at 48, 72, 90, 120, or 160 columns

Full CI-equivalent gate before done claim:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
cargo audit
actionlint .github/workflows/ci.yml
```

If a tool is unavailable locally, report the exact command, failure, and whether the blocker is environmental or scoped to this change. Do not replace these with narrower package tests when claiming full readiness.

## Security and Reliability Assumptions

Security context: applicable.

Assets / privileges:

- router SQLite state
- Codex session state references
- account/quota metadata
- provider credentials by boundary
- terminal/plain/JSON output
- telemetry labels

Entry points:

- `codex-router quota status`
- `codex-router sessions`
- persisted router state reads

Trust boundaries / auth assumptions:

- `serve` owns writes and provider refresh.
- `quota status` and `sessions` are read-only observers.
- Status display freshness does not grant runtime routing authority.

Sensitive data / privileged actions:

- provider tokens
- raw provider errors
- raw account IDs
- unsafe account labels
- DB writes/migrations
- provider HTTP

Security invariants:

- no DB writes from status or sessions
- no provider I/O from status
- no secret/raw-id/unsafe-label leakage
- low-cardinality telemetry only
- color is not the only semantic carrier

Security non-goals:

- auth redesign
- credential storage redesign
- provider refresh protocol changes
- routing-authority changes

Required proof:

- read-only DB and query-only tests
- no-provider-I/O stale fixture
- redaction tests
- telemetry contract assertions for tracing/log attributes and OTel metrics
- degraded-read fixture

Reliability constraints:

- Do not swap read-only active-client reads for mutating stale-prune reads.
- Do not refresh rollups from the status path.
- Do not call provider refresh from status.
- Degraded projection fallback remains visible and tested.
- Existing stale rows with old 300-second stale-after values render values while status sample confidence uses the 15-minute display threshold.
- Runtime routing behavior remains unchanged unless a separate routing spec is accepted.

## Risks

- The biggest risk is reintroducing routing semantics through a display-freshness change. The runtime-authority guard gate exists to catch this.
- JSON/plain output may be consumed by scripts. Keep JSON enum meanings stable and add new fields only deliberately.
- The centered burn meter can be too wide in narrow layouts. Width tests and visual captures are mandatory.
- The current worktree already has dependency/presentation diffs. Gate 0 must classify them before overlapping edits.
- Account labels may contain unsafe local values. Use the existing safe-label helper or explicitly re-scope with review.

## Open Questions

- None blocking for implementation planning. This revision chooses status-only 15-minute freshness and preserves runtime selector authority.

## Recommended Next Skill

Run `shravan-dev-workflow:implementation-execute-plan` from Gate 0.

phase_result: complete
evidence: `docs/plans/2026-07-04-quota-status-reset-pace-ux/reviews/2026-07-04-final-plan-review.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Final plan review found no remaining blocker or important findings after the telemetry proof revision.
