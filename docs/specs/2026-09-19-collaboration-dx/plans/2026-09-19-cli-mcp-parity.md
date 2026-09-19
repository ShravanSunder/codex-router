# Implement CLI/MCP collaboration parity

## Canonical planning record

- Originating planner: `plan-implementation`
- Planning result: `ready`
- Governing planning basis: `reviewed-three-artifact-design`
- Requirements: [requirements.md](../requirements.md), U1–U7 and the Rust/MCP library constraint.
- Specification: [specification.md](../specification.md), R1–R9 and V1–V7.
- Program Design: [program-design.md](../program-design.md).
- Current review: independent Astra-high native reviewer `/root/astra_dx_design_review`, fresh-context initial review and same-reviewer correction verification; subsequent owner-authorized HTTP revision reviewed with no material corrections. Inspectable review outcome: shared thread `01a0b741-6a76-7522-ae6c-72631fa7c39c`, message `01a0ba28-6ee8-7ad2-8031-ad0e48990a29`. This is a native reviewer identity, not a Router SessionRef.
- Applicability: `74aaa88ccb5fbd2b76f3a6dc8dee464551be6dc0`, branch `feat/agent-collaboration-dx-design`, clean product source based on `origin/main`; only design artifacts added.
- Requested terminal: `plan-only`.
- Delivery grouping: `single:cli-mcp-parity`.
- PR topology: `one-pr` for the foundation. External ACP providers remain a separate follow-on and are not implemented by this plan.
- Tracking: reuse the Agent Router design thread above; no new tracker.

This plan is not execution authority. It preserves the user instruction to discuss/plan without implementation.

## Goal and protected boundaries

Deliver equivalent CLI and real Streamable HTTP MCP operations using the existing collaboration SDK. Bind MCP only to loopback, without V1 authentication or TLS. Preserve Codex conversation identity, backend permissions, exact-turn interruption, board/automation ownership and uncertainty reporting.

Do not add task IDs/stores, worktree provisioning, automatic replacement, transcript storage, native-subagent control, mesh routing, external ACP providers, or custom MCP protocol machinery. Do not change the production Router process, global installs, Homebrew, or skills.

## Source owners and validation foundation

- `collaboration-protocol`: domain requests/results and schema generation.
- `collaboration-client`: existing operations; destination for reusable CLI preparation.
- `agent-collaboration`: flags, human/JSON presentation, command-to-operation mapping.
- New bounded collaboration MCP package/module: official `rmcp` adapter, schemas and HTTP dispatch; no domain state ownership.
- `codex-router-host/src/collaboration_runtime.rs`: listener composition and shutdown; supporting Host configuration and service discovery own bind settings and URL publication.
- `collaboration-service/src/agent_declaration.rs`: existing sender rendering to share at the protocol/message boundary.
- `.github/workflows/ci.yml`: repository-wide build, formatting, Clippy, nextest, SQLx metadata and dependency gates.

## Proof-bearing sequence

```text
A. SDK/dependency compatibility and operation inventory
                         |
B. Shared operations + existing CLI cutover
                         |
C. HTTP MCP + Host lifecycle integration
                         |
D. Full parity and real debug Router proof
```

These are serial dependencies within one coherent deliverable, not four separate PRs. Contracts introduced in A/B are consumed by C; no unused future-provider seam is included.

### A. Establish a compatible library and exact operation inventory

Inspect the released `rmcp` version, feature set, toolchain minimum, schema version and Tower/Hyper integration. Pin a compatible released version through workspace dependencies and lockfile; never copy upstream-main APIs without release verification. Use official helpers for initialization, HTTP/SSE framing, tool routing and cancellation. Crates.io lookup was unavailable during design; current released dependency compatibility is a required implementation-entry check, not a claimed result.

Enumerate every finite operation in the existing CLI command families named by R1. Record each input/result/error type, existing SDK handler and corresponding MCP tool. Exclude only the explicitly specified presentation/raw-transport facilities. Add contract coverage that fails when a supported domain operation lacks either adapter or exports unresolved schemas.

Write surfaces: workspace Cargo configuration, bounded MCP package manifest, shared operation/schema definitions and permanent contract tests. No product session/state migration.

Proof: compile selected SDK integration against Rust 1.98.1 and workspace types; verify generated MCP schemas resolve references; run dependency policy/audit. Consumer: B/C. Stop if satisfying protocol compatibility requires a new runtime, bespoke MCP stack or policy change; return evidence to design.

### B. Share behavior and cut the CLI over

Identify failing boundary tests before changes: target mismatch, endpoint ambiguity, explicit creator/sender, initial versus resumed prompt attribution, partial creation, and retained effect evidence. Extract reusable preparation/error classification from CLI into existing SDK owners. Preserve transport-specific formatting.

Expose separate conversation creation using the current Codex adapter. Creation returns the existing backend ID; subsequent load keeps it. Share Agent/Human declaration rendering for ordinary sends and prompt-and-wait; reject internal Router content at the public prompt boundary. Do not use creator identity as a later sender.

Add bounded observation shared by CLI/MCP, with explicit attach and no concurrent observe/send ordering promise. Retain existing streaming CLI presentation and use prompt-and-wait for correlated execution evidence. Retain cancellation/uncertainty distinctions.

Write surfaces: `collaboration-client`, necessary `collaboration-protocol` types, declaration renderer owner, CLI handlers, permanent tests in their existing suites.

Proof: focused unit tests for validation/rendering and integration tests through existing Control/ACP sockets for real routing and errors. Test doubles may force transport failures; they may not replace the recipient-visible attribution or thread identity checks. Compile CLI against the extracted shared owner; remove superseded duplicate behavior rather than keep a legacy path.

### C. Add Streamable HTTP and compose it into Host

Implement the thin `rmcp` server calling B's SDK handlers. Derive schemas/results from the same types, preserve structured domain errors, and provide protocol errors only for MCP-level invalid calls. Map cancellation according to R7: no promised late response after MCP cancellation.

Compose a separate collaboration HTTP listener into Host lifecycle with validated loopback bind configuration, local URL discovery and explicit Origin validation. Do not widen or reuse the model proxy credential boundary. Bind failure must be visible, with no stdio or external-interface fallback. Use the existing Host Tokio runtime and tracked shutdown.

Write surfaces: bounded MCP package/module, Host collaboration/configuration/discovery owners, schema metadata and permanent HTTP integration tests. No authentication store or TLS provisioning.

Earliest integration gate: actual HTTP initialize, tool discovery and one typed tool call through the real SDK and an owned test Router. Exercise localhost/IPv4/IPv6 loopback as supported by bind configuration, reject wildcard/non-loopback binds, and reject invalid Origin before any effect. Verify operation errors remain equivalent to CLI JSON. Follow the supported release's HTTP method/version/session rules rather than implement protocol behavior by hand.

Proof also covers Host listener failure, shutdown and reconnect. MCP transport session IDs are never persisted as Codex IDs. No automatic mutation replay on reconnect.

### D. Complete parity and real acceptance

Use the same scenario matrix through CLI and HTTP MCP. Normalize presentation-only differences while comparing target identity, domain result/error, generation/turn evidence and observable backend effect. Cover all catalog rows at the appropriate inexpensive layer; reserve live provider calls for representative cross-boundary journeys.

Real debug journey: discover -> create with explicit cwd/model/effort/access/caller -> inspect -> prompt/send -> observe -> exact-turn interrupt when applicable -> resume the same conversation via the other surface. Verify recipient-visible sender identity, including a second sender resuming the conversation. Exercise existing board operations through both surfaces on disposable test data. Do not infer assignment success from turn completion.

Use the existing debug Host acceptance framework and owner-controlled socket directory. Respect normal Codex home and startup serialization; do not substitute a fake home and claim production-path proof. Never restart production or run the public installer. If the debug backend is unavailable due to another owner's process, report the exact blocker rather than stop it.

Failure proof: application timeout versus MCP cancellation; no late-response assumption; unordered observe/send; result limits; malformed result after effect; backend generation change; lost connection after dispatch; no unsolicited replay or approvals.

## Requirement-to-proof coverage

| Obligations | Slice | Required observation |
| --- | --- | --- |
| R1–R2 / V1 | A–D | Exact catalog coverage, resolved input/output schemas and equivalent domain behavior across both adapters. |
| R3 / V2 | B, C, D | Service mismatch/ambiguity rejected before effect; successful targets preserve all existing ID fields. |
| R4–R5 / V3 | B, D | Same-ID create/load, effective settings, correct sender rendering and unchanged approval checks. |
| R6 / V4 | B, D | Delivery preconditions and interruption of only the specified turn. |
| R7 / V5 | B–D | Bounded observation, cancellation and unknown effects accurately reported without invented receipts/order/replay. |
| R8 / V6 | All | No additional conversation registry/transcript store or external-provider execution. |
| R9 / V7 | C, D | Real loopback HTTP MCP, no auth requirement, Origin and bind enforcement, Host lifecycle and reconnect behavior. |

## Validation commands and delivery limits

During implementation use focused `cargo check -p <affected-package> --locked` and `cargo nextest run --profile ci -p <affected-package>`; include the new MCP package once named. Run `cargo fmt --all -- --check` and affected Clippy checks after edits.

Before claiming implementation complete preserve the CI gates from `.github/workflows/ci.yml`: workspace Clippy with `-D warnings`, workspace nextest CI profile, pinned tool bootstrap tests, `python3 scripts/tooling/prepare-sqlx.py --check`, CLI default/all-feature builds, quota-reset harness checks, Homebrew release-script tests, `cargo deny check`, and `cargo audit`. Reuse CI for its installation checks; do not globally install locally to mimic them. Report exact commands, exits and test counts alongside the real debug proof.

Check retained build size and active consumers before any optional Cargo cleanup; never remove directories or clean artifacts used by another process.

The later implementation delivery requires independent code/proof review and the repository-required unused semantic version/Cargo.lock update before merge. This plan authorizes no merge, tag, release, Homebrew action or production process replacement.

## False-green risks and stop conditions

- Matching schema names does not prove matching behavior; compare effects and failure semantics.
- A fake MCP server proves neither HTTP framing nor actual tool discovery; use the real MCP adapter.
- A fake Codex peer cannot establish provider settings, permission routing or real create/detach/load behavior.
- A cancelled call with no reply is not proof that backend execution stopped.
- Endpoint discovery does not prove remote mesh routing; remote exposure is outside V1.
- If code contradicts the same-ID/no-extra-store design or requires new policy, stop writing and return the smallest evidence-backed design gap to the parent.
- If current `origin/main` changes relevant owners before execution, inspect those differences and return any semantic discrepancy to the planner rather than silently rewriting this record.
