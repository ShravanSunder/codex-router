# Router selector Program Design integration record

The earlier D1/D2 preparation is superseded by the machine/filter decision and canonical integration. The normative homes remain the three existing documents; this is a process record.

## Integrated contracts

- [Requirements](../specs/2026-10-03-router-selector/2026-10-03-router-selector-requirements.md): U8 adds explicit machine filtering, All view and shortcut/visible access; U6 reflects delivery direction while preserving protected boundaries.
- [Specification](../specs/2026-10-03-router-selector/2026-10-03-router-selector-specification.md): E8/R10 define All/individual views and source labels; R2 fixes concrete NEW placement, R3 preserves saved defaults, R9 preserves default forks and source-owned remote cwd.
- [Program Design](../specs/2026-10-03-router-selector/2026-10-03-router-selector-program-design.md): existing picker owns filter and per-source projection; dispatch owns source loader; existing protocol owns identities/inventory; action owners retain full source context through handoff.

All composes the already-bound per-source request/result/continuation contract, not a new wire endpoint or persisted catalog. Sources retain independent paging, observation guards and unavailable status. Default provenance and cwd derive from U3; no extra home/protocol qualification gate is introduced.

## Required verification

Focused verification must cover changed machine/All semantics, NEW preselection, full source identity, duplicate native IDs on different services, partial failure/cancel, shortcut ambiguity and updated U/R/E traceability. Reuse the retained Claude reviewer; carry unchanged A2 coverage forward, and do not claim a fresh whole-design verdict or runtime proof.

Actual transport/attachment/policy qualification and real two-machine proof remain separate integration requirements. Stand-ins may exercise local boundaries but never fulfill remote success. No new helper, production replacement or exposure change follows from this record.

## Focused U8 correction

The retained reviewer matched NEW preselection, per-row identity, U/R/E coverage and U3 default provenance/cwd preservation. Accepted K1–K4 corrections: use Ctrl+G without changing terminal input mode; source/home- and generation-keyed preview cache with no caller-local remote rollout; one complete-view watch snapshot plus bounded per-source async work/result scheduling in the existing runtime; deterministic alias display while Single retains its selected profile. The same reviewer verified K1–K4 closed. Its one remaining diagram-label typo (Ctrl+M instead of Ctrl+G) was corrected and parent-verified with no meaning change. No full-design/runtime readiness is inferred.
