# Router-fixes checkpoint

Current branch: `router-fixes`; product baseline: `7a8cbd89` (0.1.63). This is a design-document checkpoint, not tested implementation or release readiness.

[Requirements](../specs/2026-10-03-router-fixes/requirements.md) and [backlog](2026-10-03-router-fixes-backlog.md) are the entry points. Current design states:

- Native interruption (U2): bounded design reviewed and accepted; canonical plan remains local at `tmp/plan-workflows/2026-10-03-native-interrupt-errors.md`. [Specification](../specs/2026-10-03-router-fixes/interrupt-specification.md), [Program Design](../specs/2026-10-03-router-fixes/interrupt-program-design.md).
- Push truncation (U6): prior bounded corrections accepted; fresh different-lineage review reports ready. Canonical plan remains local at `tmp/plan-workflows/2026-10-03-push-truncation.md`. [Specification](../specs/2026-10-03-router-fixes/push-specification.md), [Program Design](../specs/2026-10-03-router-fixes/push-program-design.md). Actual recipient/consumer proof remains required.
- Provider failure (U1): [Specification](../specs/2026-10-03-router-fixes/provider-failure-specification.md) and [Program Design](../specs/2026-10-03-router-fixes/provider-failure-program-design.md) are drafts requiring review remediation: prompt retirement can overwrite positive non-dispatch evidence; producer/consumer/gate dispositions need closing. No implementation plan admitted.
- WebSocket close (U4): [Specification](../specs/2026-10-03-router-fixes/websocket-close-specification.md) and [Program Design](../specs/2026-10-03-router-fixes/websocket-close-program-design.md) are drafts requiring review remediation: retained survivor admission waits must observe cleanup intent and resource-release attribution must match source. No implementation plan admitted.
- Managed questions (U3): shared-model extension selected; [Specification](../specs/2026-10-03-router-fixes/questions-specification.md) exists. Execution-lifetime ownership decision remains pending. No final Program Design or implementation plan.

## Environment and evidence

The owner resumed delivery after migration on 2026-10-04. An actual Cargo cache write denial was corrected with the owner-approved filesystem grant; no firewall or dotfile change was made. `cargo fetch --locked` passes exit0. The first adapter check exits101 because inherited CC points at absent Homebrew LLVM. An installed Apple compiler is available; process-local compiler correction passed the adapter check with exit0. Future Cargo commands unset the stale CC/CXX/LDFLAGS/CPPFLAGS values without dotfile edits. These checks establish environment readiness only, not feature correctness.

Historical source checks: HTTP-streaming G-24 pass/exit0; G-25 fail/exit1 on obsolete marker/moved-owner scanning. Artifact whitespace and relative-link checks passed. No feature Rust test, required recipient matrix, CI, signed release or Homebrew acceptance has passed for this milestone.

Pure same-crate reorganization clearance was given for the decomposition track's 28 named oversized files while preserving behavior and proof; future overlapping writers coordinate per-owner source/test/filter maps. This does not block independent fixes surfaces. The private work trace remains `docs/wip/work-trails/2026-10-03-router-fixes/main.md`; it contains local coordination context and is intentionally excluded from this public-safe checkpoint.
