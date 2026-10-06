# Router selector admission and decision packet

Current owner direction is delivery through the Selector lane, next in the sequential merge train. This packet remains the single status entry point. Historical D1–D4 briefs explain earlier alternatives; they are not blanket approval gates.

## Settled behavior

The owner explicitly selected in-picker machine selection/filtering, requested a shortcut such as Ctrl+M for All/individual machine selection, and required launching/forking on the appropriate machine. The canonical Requirements now include U8; Specification R2/R3/R9/R10 and Program Design bind the behavior.

- **D1 resolved:** visible Machine control, Ctrl+G and F2 shortcut access; All machines or an individual machine filter. All is a transient source-tagged view. NEW confirms one concrete machine, preselected from an individual filter; existing actions retain each row's full routing identity.
- **D2 resolved by U3:** preserve default/local action behavior, distinguish default attribution from actual hosted observation, and never reinterpret an attributed ID on a configured remote source. No new default qualification failure or protocol/home contract is admitted.
- **D3 resolved by U3 and machine-affine action requirement:** preserve invoking-cwd/override semantics on existing default/local forks; configured-source fork uses qualified source cwd and shows directory/origin before confirmation. This does not authorize changing security policy or path mapping.
- **D4 is not an owner decision:** rejection-only/stand-in evidence does not complete remote success. Independent local work may progress, but every required real interaction remains a delivery gate.

Ctrl+M aliases Enter in legacy terminals: crossterm 0.29 `src/event/sys/unix/parse.rs:92–94` maps carriage return to unmodified Enter. Iocraft currently requests only REPORT_EVENT_TYPES, so the suggested Ctrl+M is not a usable distinct shortcut. The Lead selects Ctrl+G (currently unassigned, correctly decoded by crossterm) with F2/visible access and no terminal-mode change. Ctrl+M is never advertised as a machine action.

## Current evidence

Published decomposition #127 is integrated at `41671e86ba9927b29cafc562ad98a0f0dea84e8f`, without conflicts or selector-document loss. Product checkpoint `6816ea56` preserves source identity and adds read-only JSONC registry/machine controls. Continuation `5bde8dae` adds async source requests, bounded latest-view scheduling, explicit provenance and a source-affine fork confirmation with effective directory metadata. [Draft Selector PR #128](https://github.com/ShravanSunder/codex-router/pull/128) now publishes this partial implementation. No remote success or merge readiness is claimed.

The existing full routing foundation is `EndpointRef`, `SessionRef`, and `SessionPickerIdentity`. Picker outcomes and dispatch now carry `SessionActionSelection` with full identity, source context/provenance and selected model/effort. Native endpoint resolution rejects mismatched service/endpoint before selecting a socket. Preview requests/cache include source/home/generation. No new persisted identity schema was introduced.

Current author proof: source-binding/metadata/query/alias continuation has agent library263 tests,262 passed/0 failed/1 pre-existing ignored, exit0. Full agent and quota-harness affected-package suites passed exit0, including actual compiled PTY14/14. Final workspace/all-target Clippy with denied warnings, formatting and whitespace passed. The actual source-owned Control/SQLite paging proof now integrates endpoint binding through full picker summary projection: full target/model/effort/cwd/source classification/Stored timestamp, source-specific metadata, unchanged database bytes and zero native socket connections. Source paths never resolve against invoking-machine symlinks. Default stored/inspected metadata priority and local catalog normalization remain unchanged. Permanent expected-red tests established foreign discovery reaching an extra session read, invoking-machine canonicalization being unsuitable for source rows, unsupported source queries lacking an explicit reason, and empty unbound configured results being wrongly accepted. The final library was refreshed after moving binding-client tests into their adjacent responsibility module.

Configured-source Ready now requires an explicit full bound endpoint even with zero rows. Publication rejects wrong service/endpoint, attributed/local-history and invoking-machine-normalized records. Qualified alias presentation uses registry order plus actual returned bindings: All labels the first name plus aliases, including an empty first inventory; Single retains its selected name and routing profile. Real iocraft rendering proves the display against simulated bound replies, not remote inventory/transport qualification. Local endpoint binding and query qualification are implemented; source-owned Cwd/Repo/provider contracts and the common search expression are explicitly unsupported rather than silently broadened. Any/empty source queries still reject unqualified transport.

Published head `a0204644` has all five CI checks successful (run37415622908); the bounded watch ended by interruption and the subsequent exit0 PR snapshot establishes this result. The new local checkpoint has fresh local gates above; its publication/current-head CI is recorded separately in the trace and PR. Prior head CI is never promoted to new source changes.

Open stand-in: configured real profiles still reject before credentials/network/native effects until an established read exposure/auth contract, native attachment binding and permitted policy projection are supplied. The existing unanswered connection request is refined in the local-only connection/proof packet; no repeated question or guessed route is submitted. Configured producer integration, one real paged read per qualified alias group, remote per-page progress and real NEW/resume/fork/cancel/reconnect/post-handoff failure remain unverified. Current qualified alias publication shares the transient displayed inventory; it does not prove reduced remote reads. Native creation, real tailnet/multiple app-server configs, whole-requirement Lead assessment, retained Advisor/distinct final review and release/version gates remain open. The PR stays a partial draft.

Retained independent Claude review closed A1/A3/A4 and the focused A2/Program Design corrections, including final action-metadata ownership, with no residual at that check. Focused U8 verification found three substantive design gaps (Ctrl+M advertisement, bare-ID preview cache, and watch-channel fan-out) plus an alias-label ambiguity. The Lead inspected decisive sources and corrected them; the same reviewer verified K1–K4 closed, with one Ctrl+M diagram-label leftover; the Lead corrected that non-semantic label to Ctrl+G and verified no stale shortcut edge remains. Prior coverage is not silently extended. Reasoning effort remains unavailable.

## Remaining delivery work

| Item | Actual state | Owner of next work |
| --- | --- | --- |
| Changed machine/All contracts | Authored and scoped checks pass; K1–K4 independently verified closed, diagram typo corrected; whole-design readiness not claimed | Selector Lead and same retained reviewer |
| Implementation plan/product path | Independent frontier plans execute under explicit owner stop-review continuation; source isolation, registry/controls and async/fork paths implemented with scoped author proof; whole-lane readiness not claimed | Selector Lead |
| Remote exposure, attachment identity and policy projection | Existing source/transport comparison only; no concrete qualified route or remote runtime proof | Resolve from supplied exact exposure/authorization; do not invent a gateway or remote policy |
| Real proof | Local SQLite/history files, renderer behavior, isolated service resolution and default dry-run verified; actual selected tailnet machine/native creation/fork, multi-app-server, auth/policy, reconnect and post-handoff failure remain unverified | Implementation lane, personally verified by Lead |
| Final delivery | Required Advisor after Lead proof assessment, distinct implementation review, full CI and exact PR state; merge only after gates | Existing delivery/review relationships |

## Retained references

- [Requirements](../specs/2026-10-03-router-selector/2026-10-03-router-selector-requirements.md)
- [Specification](../specs/2026-10-03-router-selector/2026-10-03-router-selector-specification.md)
- [Program Design](../specs/2026-10-03-router-selector/2026-10-03-router-selector-program-design.md)
- [Review disposition](2026-10-04-router-selector-formal-review-result.md)
- [Source-contract preparation](2026-10-04-router-selector-a2-contract-preparation.md)
- [Transport comparison](2026-10-04-router-selector-tailnet-endpoint-brief.md)
- [Trace](work-trails/2026-10-03-router-selector/main.md)

No production restart, credential/configuration change or remote exposure is authorized by this packet. New session state remains on the selected machine; a transient All view copies no state and enables no cross-machine fork.
