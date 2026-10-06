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

Published decomposition #127 is integrated at `41671e86ba9927b29cafc562ad98a0f0dea84e8f`, without conflicts or selector-document loss. Its size scan checked 1707 Rust files, none over 1000 lines; checker tests passed 16/16. Current selector edits remain documentation only. No Selector product implementation or PR exists.

The existing full routing foundation is `EndpointRef`, `SessionRef`, and `SessionPickerIdentity`. The narrow code gap remains `SessionsPickerOutcome::{ResumeSession,ForkSession}(String)` and dispatch's bare-ID/local-metadata resolution. The Program Design carries the existing identity through source-aware actions, without a new persisted identity schema.

Retained independent Claude review closed A1/A3/A4 and the focused A2/Program Design corrections, including final action-metadata ownership, with no residual at that check. Focused U8 verification found three substantive design gaps (Ctrl+M advertisement, bare-ID preview cache, and watch-channel fan-out) plus an alias-label ambiguity. The Lead inspected decisive sources and corrected them; the same reviewer verified K1–K4 closed, with one Ctrl+M diagram-label leftover; the Lead corrected that non-semantic label to Ctrl+G and verified no stale shortcut edge remains. Prior coverage is not silently extended. Reasoning effort remains unavailable.

## Remaining delivery work

| Item | Actual state | Owner of next work |
| --- | --- | --- |
| Changed machine/All contracts | Authored and scoped checks pass; K1–K4 independently verified closed, diagram typo corrected; whole-design readiness not claimed | Selector Lead and same retained reviewer |
| Implementation plan/product path | No admitted ready plan or implementation yet; do not claim PR readiness | Selector Lead after affected contracts are checked |
| Remote exposure, attachment identity and policy projection | Existing source/transport comparison only; no concrete qualified route or remote runtime proof | Resolve from supplied exact exposure/authorization; do not invent a gateway or remote policy |
| Real proof | Selector/multi-app-server/tailnet, auth/policy isolation, resume/fork/cancel/reconnect/failure and local regressions unrun | Implementation lane, personally verified by Lead |
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
