# Host-controller foundation — plan draft

This is the owner-requested Phase 0 draft for Program Design §6.10's unaffected
group. It is **not ready for implementation**. Final planning waits for the
coordinator's `design READY at <sha>`, current review evidence, and a working
pinned toolchain. No product implementation is authorized by this document.

## Basis and delivery boundary

- Originating planner: `plan-implementation`, authored by the host-controller Lead.
- Classification: `general-domain`.
- Planned against: `host-controller-impl`, source base
  `9e947528a5122fbd09745bbd36fa0548a6daf14f` (`origin/main`).
- Design input: `origin/design/host-controller` at
  `a43c70cc1405f8274fa99bcfa4d167ce9c12fcdc`, all three artifacts under
  `docs/specs/2026-09-26-host-controller/` read completely. That snapshot is
  undergoing re-anchor; historical readiness does not admit implementation.
- Phase result: `blocked` for canonical planning; canonical plan identity: none.
- Current terminal: the Phase 0 draft and checkpoint. Eventual delivery proceeds
  through implementation, review and `pr-ready-unmerged`; merge and tag creation
  retain their explicit owner gates. Release publication is separate from
  production process replacement.
- Tracking: existing coordination thread, mirrored by the coordinator; local
  unshared trace until its service is reachable. No extra ticket system.

The eventual ready delivery plan will have exactly one canonical home under
`tmp/plan-workflows/`, following the installed planning skill. The current
project ignore policy already covers that path. This working draft remains
separate from that authority.

## Mental model

The keeper owns singleton authority, listener lifetime and managed generation
identity. Restartable roles receive owned descriptor duplicates and perform
request-serving effects. Direct Codex clients resolve E1 to a generation alias;
an already-open connection continues to the generation it reached. Services
admission carries generation, alias and schema evidence together.

A generation transition has one candidate-or-predecessor slot, in addition to
the current generation. Publication is the irreversible commit point. Before
publication, any failure preserves the predecessor and E1. After publication,
services must commit the new generation before the old generation can retire.
Failed services recovery retains the predecessor and keeps the transition busy.
The one-second settle protects handshakes already accepted by the predecessor;
it is distinct from the first-request interruption budget and group-stop bounds.

Self-exec preserves process identity, listeners and children; it does not transfer
live application work into the keeper. Descriptor transport and process ownership
must be proven separately from application-state continuity.

## Scope and first consumers

Draft only:

1. `codex-router-keeper-protocol` and `codex-router-keeper` crate boundaries.
2. ListenerRegistry, typed descriptor grants and KeeperChannel framing.
3. GenerationController, generation-specific launch/evidence, atomic E1 swap,
   settle, owned-group retirement and the transition invariant.
4. Fingerprint construction and validation mechanics for settled role inputs.
   Role integration and the proxy closure wait for the revised design.

The first downstream vertical consumer is the complete `host` keeper cutover:
real collaboration/proxy/provider roles adopt the grants, the generation gate
consumes the staging/commit/retire messages, and the CLI/operator update path uses
BuildInfo. Foundation APIs must serve those named consumers, rather than create
another Host mode or compatibility path.

Deferred: child runtime Prepare/Activate effects, detached-turn transfer/rejoin,
Codex State emission, ProviderLink query/operation semantics, proxy credential
drain, role renames, command removals, actual keeper self-exec integration,
Desktop reconcile and release execution. Their proof obligations remain intact.

## Current source anchors

| Concern | Current source | Implication for the draft |
| --- | --- | --- |
| Singleton and operator listener | `crates/codex-router-host/src/host_singleton_authority.rs:18` | Today one object owns both; acquisition binds by path, and v1 exec carries only the lock. Preserve authority continuously and separate listener adoption from fresh binding. |
| Private Unix listener cleanup | `crates/collaboration-service/src/private_socket_listener.rs:14` | Private parent, socket permissions and inode-scoped unlink are existing behavior. A child constructed from a grant must never gain pathname cleanup ownership. |
| Launch projection | `crates/codex-native-integration/src/app_server_launch.rs:22` | Preserve root permission/network overrides, debug precedence and Remote Control policy when substituting the generation alias. |
| Executable/schema preparation | `crates/codex-router-host/src/managed_app_server.rs:99` | Extract the existing identity, export and readiness responsibilities to native integration as specified; do not import the Host crate into the keeper. Existing export degradation must be reconciled with revised generation-evidence rules. |
| Existing retained child | `crates/codex-router-host/src/process_group_child.rs:118` | It owns a Tokio Child today. Reuse launch configuration and semantics, not a Tokio reaper that cannot be reconstructed after exec. Each keeper-owned PID has one reap owner. |
| Admission gate | `crates/collaboration-service/src/native_generation_gate.rs:43` | Today activate requires the previous admission retired. New stage/commit/abandon/retire wiring is a downstream integration obligation. |
| Lazy Codex route | `crates/codex-acp-adapter/src/lazy_codex_session_route.rs:84` | Admission is currently one ActiveGeneration per connection route, shared by its sessions; revised design must describe this actual consumer. |
| Detached prompt lifetime | `crates/codex-acp-adapter/src/session_connection_registry.rs:206` | Busy frontend loss detaches onto Host-lifetime tasks; no return to cancel-on-disconnect behavior is allowed. |
| Active Codex load | `crates/codex-acp-adapter/src/session_creation.rs:513` | Active returns Busy today. Foundation or codec proof cannot establish live-turn rejoin. |
| Compiled acceptance isolation | `crates/codex-router-cli/tests/compiled_cli_host_acceptance.rs:1` | Keychain-test-support is mandatory. Extend its temporary-install/isolation harness; retain its launch-policy and singleton checks. |
| Repo completion gates | `.github/workflows/ci.yml:33` | Keep formatting, dependency policy/audit, script checks, Clippy, SQLx metadata, both CLI feature builds and both Nextest suites. |

## Proposed milestone and task graph

One foundation milestone/PR groups contracts with the real descriptor and
filesystem behavior that consumes them. Separate transport-only PRs would expose
half-consumed contracts and add review overhead. Full user-facing cutover is a
later milestone; its exact grouping belongs to planning after READY.

```text
F1 protocol/crate boundaries
  ├─ F2 framed channel and descriptor receipt ─ F3 listener grants
  └─ F4 generation launch/publication/retirement
F1 + settled role-input contract ─ F5 fingerprint construction
F2 + F3 + F4 + F5 ─ F6 foundation integration/proof
F6 ─ downstream real-role cutover and keeper self-exec
```

Shared workspace manifests, exports and lockfile changes are serial integration
surfaces. Only disjoint module work can run concurrently after those contracts
are settled. The Lead keeps plan authorship, grouping and acceptance. A persistent
implementation Sidekick owns the milestone after admission; bounded Workers may
own disjoint tasks. Use the owner's Luna xhigh/Sol high policy. Independent review
uses the authorized review models and fresh source packets; cross-lineage review
must be coordinated if unavailable on this host.

| Task | Type and write surfaces | Scenario and independent oracle | Stop condition |
| --- | --- | --- | --- |
| F1: protocol and crate boundaries | Contract: two new crates, workspace membership/dependencies, checked wire-to-domain conversion. First consumers F2–F5. | Invalid IDs, mismatched epoch/alias, out-of-order generations and invalid fd manifests fail before effects; exhaustive variant round trips and dependency-graph assertions. | A revised schema or dependency owner is unsettled; do not fossilize it. |
| F2: KeeperChannel | Contract + integration: framing/receipt modules in the protocol crate, rustix `net`, channel tests. | Real socketpairs transfer owned fds; split headers/payloads, coalesced frames, short writes, EOF, truncation, unexpected rights/counts and maximum limits fail or complete as specified. Rights are sent once and associated with the correct frame. Receive sets CLOEXEC before any allowed spawn. | Safe APIs, macOS spawn-gate coverage or self-exec payload queuing are unproven. No raw-fd unsafe conversion or new handoff mechanism. |
| F3: ListenerRegistry | Integration: keeper registry, grants and permanent process tests. | Keeper-held Unix/TCP listeners remain connectable while granted duplicates are dropped/replaced. Path inode and private permissions stay stable; duplicate RequestListener returns the existing endpoint. A real fixture child accepts and answers, not just connects. | Dynamic endpoint identities or pathname cleanup move to a child. |
| F4: GenerationController | Prefactoring + vertical filesystem/process behavior: native launch/schema/probe extraction; keeper generation/stop modules and tests. Downstream consumer is real generation-gate cutover. | Candidate launch does not alter E1; a ready staged candidate is published with rename; previous accepted connection still answers during settle; failed readiness/staging/rename preserves N/E1. Concurrent transition returns Busy, two-live bound holds, group-empty precedes retirement completion. | Current Codex alias/drop-guard behavior differs from the reviewed pin, a service-commit assumption changes, or failed recovery would retire N while it is still admitted. |
| F5: fingerprints | Build integration: CLI build.rs/support module, build dependencies and permanent tests. | Independent builds differing in one input class produce the required role-specific BuildInfo difference; version/metadata-only and excluded-file edits do not. Shared-crate, external lock/checksum, toolchain/profile/feature and included-asset edits move all affected settled closures. New/deleted inputs trigger rebuild. | Revised §7 does not settle an input/entrypoint/closure. Proxy closure is deferred; do not substitute today's serve closure as its final contract. |
| F6: integration and assessment | Integration/proof: permanent fixtures and tests, dependency assertions, CLI build-info consumer when admitted by READY. | Real compiled roles/fixtures consume the same protocol and grants; no leaked authority fds after initial or later grants. Source/diff and proof map match named boundaries. | A stand-in is presented as final V proof, an old command gains a shim, or a required proof is weakened. |

For TDD, first identify or add permanent scenarios at the owning boundary and
confirm the expected pre-change failure. Existing characterization tests for
extractions should pass before moving code. No disposable test files.

Use properties or tables for framing partitioning, role-input classification,
validated identities and state transitions. Use measured process/filesystem
journeys for fd ownership, publication and reaping. The independent observations
are received responses, descriptor flags/inventory, process identities, stable
inode/readlink, bounded elapsed time, group absence and deliberate build-input
differences—not the implementation's own status alone.

## Proof obligations retained

| Gate | Foundation contribution | Required later completion |
| --- | --- | --- |
| V1 | Stable generation/listener identities | Real direct and hosted turns across E4 replacement, same turn and terminal State, approval servicing and no-content completion. |
| V2 | Real socket/response probes, rename, accepted-connection settle | Pinned current Codex/TUI no-fallback and handshake distribution, all role replacements, enabled Remote Control fence/pairing. |
| V3 | Independent fingerprint builds and persistent TCP listener | Real per-role UpdateOutcome/PID decision, proxy first response, final proxy closure including its moved workers. |
| V4 | Candidate/readiness/staging/publication failures | Real services rejection, commit-not-applied/ack-lost plus failed recovery, retained N and truthful partial result. |
| V5 | Owned-group TERM/KILL/empty observation | All roles including provider subgroups, parent-exits/descendant-lives and EOF-ignoring providers. |
| V6 | Foundation ownership isolation | Real services crash/respawn and functional ACP with E2/E1/E10 unchanged. |
| V7 | Checked frames/results | Complete hard command cutover, Busy/partial/unknown outcomes, Codex no-change and full-restart failure. |
| V8 | Transport/startup measurement seams | Fresh-image first response, forced handover, realistic stored state, schema-changing replacement and durable credential settlement. |
| V9 | Keeper-held operator listener | Status responses throughout preparation, handover, settle, crash recovery and exec. |
| V10 | Descriptor transport, singleton/ownership validation scenarios | Real self-exec on macOS/Linux, retained children/turn, fallback, failed-exec resume, quiesce/held output, late grant fd inventory and unrelated PID never signalled. |
| V11 | Provider listener identity only | Real Claude and Cursor survival, replay gaps, operation ledger and pending/idempotent decisions; provider replacement/loss. |
| V12 | No contribution in this milestone | Isolated fixture action/isolation plus separately owner-authorized real Desktop/iPhone outcome. |

No complete V gate is claimed by foundation-only proof. All remain delivery
obligations. Preserve existing tests (`keep`); repair paths/fixtures only as a
consequence of the accepted extraction. No test removal is planned. Update
obsolete command assertions only in the later hard-cutover milestone, with
replacement proof for the new public contract.

## Stand-ins and false-green risks

A protocol-speaking app-server/process fixture is allowed only at the design's
existing acceptance seam. A services peer may exercise the already named
PrepareGeneration/CommitGeneration/RetireGeneration messages before real role
integration. Its assumption is that the actual consumer honors those contracts.
It permits foundational progress; it does not prove approval ownership, live
turn continuation, application-store exclusivity or actual Codex fallback.
Those stand-ins close only with real downstream integration and its V gates.
No stand-in has been introduced in Phase 0.

Raw successful connects do not prove first-response availability. A parent exit
does not prove its group empty. A valid JSON frame does not establish authority.
A codec round trip does not establish an emitting runtime. Stable TCP binding
does not prove usable credentials. A synthetically different hash does not prove
build invalidation. A signed/warmed executable does not establish timing on a
newly installed image. Each corresponding runtime/build oracle stays required.

## Validation route and remaining prerequisites

After READY and toolchain recovery, use `/opt/homebrew/opt/rustup/bin` first on
PATH. Start with `cargo check -p <affected-package>` and narrow package tests;
do not build or install production binaries to validate this draft. Keep
`SQLX_OFFLINE=true`, repo debug signing and private debug roots/socket parents.

At milestone completion, run the repository-required gates from the workspace
root: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, dependency deny/audit,
script validation and `python3 scripts/tooling/prepare-sqlx.py --check`; build
both CLI feature variants; run the CI-profile Nextest workspace and quota-reset
suites with `codex-router-cli/keychain-test-support` as defined in CI. CI's
isolated install-artifact check remains a CI gate; never cargo-install binaries
into the developer's normal PATH. Add current Linux and macOS evidence for SCM_RIGHTS,
CLOEXEC, exec, process groups and symlink publication. Record commands, counts,
exit codes and artifact/source identities.

Before canonical planning:

- Receive exact revised design READY and its current three-artifact review.
- Re-read changed artifact sections and check applicability to current main.
- Settle self-exec handoff payload capacity and short-send completion.
- Settle detached-turn/approval ownership transfer and active-load State emission.
- Settle read-only Prepare and migration/activation costs, ProviderLink query
  coverage and final role fingerprint closures.
- Confirm current Codex compatibility evidence and preserve all V1–V12 gates.
- Recover Rust 1.98.1 through the owner's firewall authorization.

The Phase 0 checkpoint records the observed toolchain failure and open questions
outside this public repository. Implementation, runtime acceptance, independent
implementation review, PR readiness and release remain unverified.
