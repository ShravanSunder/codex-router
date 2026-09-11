# Rust reliability improvements: requirements

## Goal and boundary

Give codex-router maintainers and coding agents build-time feedback on fixed
application SQL, using the existing SQLx/SQLite foundation. Router users should
observe the same account/state behavior after adoption.

Scope is fixed application queries against router-owned SQLite state, beginning
with a coherent account/state group. The program design must identify that
group and account for remaining fixed queries; a first group must not be
described as repository-wide conversion. Dynamic SQL, migration operations,
schema inspection, and externally owned Codex databases are outside the initial
checked-query coverage. Their existing runtime validation remains necessary.

Native SQLx migration files replace the handwritten Rust migration system, as
explicitly directed by the owner on 2026-09-10. Existing data must survive the
transition. No separate schema package is required.

The build workflow and migration machinery may change, but this work does not authorize changing stored
data formats, changing routing behavior or
accessing production databases to prepare builds. The additional scope confirmed
on 2026-09-10 includes targeted Proptest coverage and CI tool-version pinning.
Nextest workflow improvements are also included by explicit owner instruction.
Targeted mutation testing and diagnostic coverage are included by owner approval;
neither imposes a blanket score threshold or replaces existing proof gates. Bacon, compiler upgrades and performance optimization are not selected
by the current design.

Existing behavior remains the baseline for this engineering change: account
login/refresh and selection, quota reporting, proxy forwarding, CLI/TUI, host
lifecycle, and agent communication must keep their current contracts. Their
code is not expanded by this work. Critical persisted account identity,
credential-generation links, enabled state, labels, routing policies and affinity
must survive adoption. Secret storage and Codex-owned session data remain untouched.

Automation storage is included following the main-branch update. It remains a
separate database owned by `automation-storage`. Use native SQLx migrations for
its existing SQL schema; keep this new database's adoption simple, without the
account database's historical conversion machinery. Preserve any existing domain
rows; this scope does not authorize data deletion or resetting execution evidence.

## Needs

Authority: the owner's SQLx adoption discussion and request for a lightweight
SQLx design in this conversation on 2026-09-09, followed by the explicit request
to capture Requirements and Specification. U1–U10 are authorized parts of that
bounded capture. All are required within scope; priority is assigned by the
owner's SQLx-first choice and preservation constraints.

| ID | Affected class | Need and reason | Basis |
| --- | --- | --- | --- |
| U1 | Maintainers and coding agents | Detect invalid fixed SQL and incompatible result shapes before executing the application. | Owner selected SQLx compile-time checking as the first improvement. |
| U2 | Contributors and CI maintainers | Build from a checkout without a running application database, while detecting stale query metadata in CI. | Adopted offline-build and metadata-freshness scope from the SQLx discussion. |
| U3 | Router users and maintainers | Preserve account/state results, transactions, migration compatibility, and existing error behavior. | Adoption improves checking rather than changing product behavior; repository proof and production-state boundaries apply. |
| U5 | Maintainers and router users | Use native SQLx migrations as the single schema history while preserving existing data. | Owner explicitly rejected retaining handwritten migrations on 2026-09-10. |
| U4 | Maintainers | Keep adoption bounded and its checked versus runtime-only coverage explicit. | Owner requested a simple SQLx design; discussion distinguishes fixed SQL from dynamic and migration SQL. |
| U6 | Maintainers and coding agents | Find protocol and selection edge cases beyond handwritten examples, with reproducible failures. | Owner requested the missing Proptest design on 2026-09-10. |
| U7 | Contributors and CI maintainers | Run the same selected tool versions regardless of cache state, with deliberate upgrades. | Owner requested the missing CI tool-version-pinning design on 2026-09-10. |
| U8 | Contributors and CI maintainers | Improve focused and full Nextest feedback while retaining all required test coverage. | Owner explicitly requested Nextest changes in the program design. |
| U9 | Schedule authors, agents and operators | Move the new automation database to native SQLx migrations while keeping schedules, runs, deliveries and their evidence intact. | Owner included automation storage and directed a simple design on 2026-09-10. |
| U10 | Maintainers | Assess whether assertions detect broken behavior and identify unexecuted paths, without treating scores as correctness. | Owner approved targeted mutation testing and diagnostic coverage with these limits. |

## Developer journey

```text
Edit a fixed query or its schema
  -> build: detect SQL/schema mistakes before application execution (U1)
  -> refresh build metadata when needed (U2)
  -> CI: reject metadata inconsistent with the current schema (U2)
  -> exercise real SQLite behavior before delivery (U3)
```

Current evidence: [storage queries](../../../crates/codex-router-state/src/sqlite.rs)
use runtime SQLx calls; [CI](../../../.github/workflows/ci.yml) runs existing Rust
checks and integration tests without a SQLx metadata freshness gate. This shows
where errors can escape compilation, not evidence of a particular SQL defect.

Success requires real build rejection and SQLite behavior evidence, not merely
adding a macro dependency. No compile-speed improvement is promised. Exact
query coverage and the schema-preparation mechanism remain structural design
work; no further product decision is currently identified.
