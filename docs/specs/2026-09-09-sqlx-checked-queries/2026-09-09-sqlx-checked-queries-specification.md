# Rust reliability improvements: specification

Governing scope and accepted needs: [Requirements, U1–U10](2026-09-09-sqlx-checked-queries-requirements.md).
This contract concerns developer build feedback and preservation of existing
runtime behavior.

## Observable contract

| ID | Required behavior | Failure or boundary behavior | Basis |
| --- | --- | --- | --- |
| R1 | Each fixed query declared in the adopted group must receive SQLx compile-time checking against the router schema, including SQL validity and the result shape supported by SQLx's SQLite checker. | An unknown table/column or an incompatible checked Rust result mapping must fail compilation. This does not promise static verification of all SQLite value conversions or business rules. | U1 |
| R2 | Ordinary builds with current committed query metadata must work without a live schema database or production credentials. | Missing metadata for an adopted query must fail with actionable build feedback rather than silently disabling checking. Offline refers to schema access, not downloading Cargo dependencies. | U2 |
| R3 | CI must detect query metadata inconsistent with the current query source and schema produced by the repository's migrations. | Stale metadata or failure to prepare the schema must fail the check. Offline compilation alone must not count as freshness evidence. | U2 |
| R4 | Conversion must preserve account/state results, null handling, domain validation, transaction boundaries, supported migration behavior, and caller-visible runtime error semantics. | Existing invalid-data, rollback, and migration scenarios must retain their expected outcomes; a successful build is insufficient evidence. | U3 |
| R6 | Native SQLx migration files must create fresh router databases and govern future upgrades. Existing supported databases must retain data through adoption; unknown or inconsistent schemas must be rejected without destructive repair. | Handwritten runtime migration dispatch must be removed at cutover. SQLx history must not be adopted solely from an unverified legacy version number. | U5 |
| R5 | The adopted fixed-query group and all runtime-only exceptions within that group must be explicitly identifiable. | Dynamic SQL, migration/schema inspection, and external Codex database queries must not be represented as compile-time checked. Leaving eligible queries outside a first group must remain visible as remaining coverage. | U4 |
| R7 | Property tests must exercise bounded generated frame fragmentation and selected domain invariants through actual implementations, with replayable failing cases. | Failures must fail the test run; automatic retry must not turn a failed property green. Existing examples and real integration tests remain. Generated tests do not claim exhaustive concurrency proof. | U6 |
| R8 | CI and the documented local bootstrap must resolve exact declared Nextest, cargo-deny and cargo-audit versions. | A cached executable of another version must not be accepted. Installation/version verification failure must stop the gate. Upgrades are explicit repository changes; advisory data continues to refresh normally. | U7 |
| R9 | Nextest must provide focused developer runs and a complete CI run, with observable failures and bounded handling of slow tests. | Focused selection must not replace required full coverage. Property failures are not hidden by retries. Resource restrictions must correspond to demonstrated shared resources. | U8 |
| R10 | Automation storage must use its own native SQLx migration set without merging with account/state storage. Existing v1 domain rows and their identities/evidence remain unchanged during adoption. | Initialization/adoption must not fire work, retry delivery, clear uncertainty, reset timing or delete history. Existing storage error categories and transaction guarantees remain. | U9 |
| R11 | Targeted mutation runs and package-level coverage reports must distinguish detected mutations, surviving or unassessed mutations, executed paths and instrumentation gaps. | No blanket mutation score or coverage percentage is an acceptance gate. Neither report replaces existing tests or proves requirements correct. Tool/run failures must not be reported as successful analysis. | U10 |

Build preparation and verification must use disposable schema state without
reading or mutating production router/Codex data. Generated metadata must not
contain production records or resolved secrets. SQLx checking does not replace
the existing SQLite integration tests or authorize weakening existing gates.

R4 applies to existing consumers, not only the three converted queries. Preserve
account authentication/refresh, selection and quota outcomes, byte-preserving
proxy forwarding, CLI/TUI behavior, host lifecycle and agent communication
contracts. Existing repository tests and real-path gates remain required; no
unrelated feature change or weakening of validation is authorized. Critical
account identity and credential-generation links, status/labels, policy values
and affinity records retain their values. Secret storage and Codex session data
are not migrated. Baseline defects outside this change must be reported separately,
not silently fixed or counted as regressions caused by this work.

## Developer-facing context

```text
Contributor -- build source + metadata --> [SQLx build workflow]
Contributor <-- success / diagnostics --- [opaque system]
CI runner ---- current source/schema ---> [same workflow]
CI runner <--- freshness pass / failure -- [same workflow]

Production router and Codex databases: outside build preparation.
Router users: runtime behavior preserved under R4.
```

## Coverage and proof

| Need | Current problem -> outcome | Contract | Required evidence |
| --- | --- | --- | --- |
| U1 | P1: SQL strings escape compilation -> O1: earlier SQL feedback | R1 | V1: actual compiler rejection of invalid SQL and an incompatible checked result mapping, alongside a valid build. |
| U2 | P2: checking needs schema knowledge -> O2: offline builds with verified freshness | R2, R3 | V2: build with no schema database available; missing/stale metadata fails at its respective boundary; current metadata passes against a freshly migrated disposable database. |
| U3 | P3: conversion can alter decoding or effects -> O3: preserved behavior | R4 | V3: real SQLite account/state and migration tests, including null/invalid data and transactional failure cases relevant to the converted group. |
| U5 | P5: handwritten schema history -> O5: native SQLx ownership with data preserved | R6 | V5: fresh initialization, supported legacy adoption with preserved records, rejection of incompatible schemas, interrupted transition recovery and concurrent-start behavior against real SQLite. |
| U4 | P4: partial checking can be mistaken for full coverage -> O4: honest adoption scope | R5 | V4: source-backed inventory matching the adopted group and explicit runtime-only exclusions. |
| U6 | P6: fixed examples miss input combinations -> O6: reproducible generated coverage | R7 | V6: generated cases execute real domain/decoder code; a deliberate invariant violation fails, and its seed replays. |
| U7 | P7: cache-dependent tools vary -> O7: reproducible tool selection | R8 | V7: empty cache, correct cache and wrong-version cache all resolve the declared versions; unavailable versions fail visibly. |
| U8 | P8: undifferentiated test feedback -> O8: focused feedback and complete CI evidence | R9 | V8: compare discovered versus executed test sets, observe deliberate failure/slow-test reporting, and exercise required real fixtures. |
| U9 | P9: new automation schema uses handwritten version bookkeeping -> O9: native migrations with unchanged domain state | R10 | V9: empty initialization, existing-v1 adoption and reopen with preserved values; existing automation replay, occupancy and delivery-recovery tests. |
| U10 | P10: passing tests can hide weak assertions and untouched paths -> O10: actionable diagnostic evidence | R11 | V10: bounded real mutation run and instrumented report with scope, failures and uninstrumented boundaries explicitly identified. |

The structural design owns how schema preparation avoids a build dependency
cycle and how checked results enter existing domain validation. This document
does not choose new components or implementation order.

## Legacy compatibility boundary for R6

The production boundary is the async state store: empty databases and recognized
legacy schemas with `user_version` 0 or 7–13. Versions 1–6 are not newly admitted
because older synchronous fixture helpers can read them. Other versions reject.
Version alone never authorizes adoption: existing router-owned objects must match
a source-backed shape and all existing records must be retained.

Recognized incomplete shapes include the partial-v10 active-session fixture and
missing auxiliary objects which current initialization creates. Missing objects
may be created empty; existing objects and records must not be replaced, guessed,
or fabricated to satisfy the target schema. In particular, absent account records
must not be synthesized from leases. A partially initialized v0 is admissible only
when its existing objects match a prefix of the old fresh-initialization path.
Unrelated tables remain unchanged. Conflicting router object definitions reject.

A one-time legacy adoption operation is necessary to preserve these existing
inputs while replacing the old migration system. It must stop owning upgrades
once SQLx history exists. It must not become a second future migration history.
This realizes preservation under U3/U5 rather than introducing support for new
legacy formats.

The supported shape set is defined by the existing async initialization SQL and
its explicit conditional additions/rebuilds, not arbitrary databases that happened
to open successfully. Fixtures for each admitted class are required evidence;
absent fixture coverage is not permission to remove that class.

R4 also preserves currently valid legacy read-only and weekly-floor access before
SQLx adoption: absence of SQLx history alone must not fail an otherwise compatible
open. Those paths do not create migration history. Writable startup owns adoption;
afterward native history and schema compatibility determine admissibility.

## Automation boundary

R10 applies to the new automation database only: empty initialization and adoption
of its existing v1 schema. Do not apply the account database's v0/v7–v13 repair
rules to automation. Current instruction revisions, schedule definitions/timing,
thread bindings, runs and summaries, wakes/deliveries, operation receipts and
event sequence identities remain authoritative. The scheduled-workflow
[Requirements](../2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-requirements.md)
retain ownership of their behavior; this work changes schema management only.

## Lifecycle journal boundary under R6

R6 includes `lifecycle-observation` and its `session-registry.sqlite` database.
Fresh initialization and future schema upgrades use native SQLx migrations.
Adoption of the existing five-table journal preserves journal identity, metadata,
record sequences and payloads, retention accounting, address rows and checkpoint
state. It must not reset replay cursors, emit new observations or rebuild domain
rows as a side effect. Known legacy schema and version are validated before
registering SQLx history; incompatible schema, version or native history fails
without committing changes. Concurrent startup must not create competing journal
identities or duplicate baseline history.

Migration completeness requires an inventory of every production SQLite schema
owner. The current owners are account state, automation and lifecycle journal.
External Codex databases remain read-only; synchronous account fixture helpers
are test infrastructure, not another production migration owner. One-time legacy
adoption ends when native history exists; future upgrades belong only to SQLx.
