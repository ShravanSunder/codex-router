`needs-revision`. The nine-table model is viable, but the documents leave several required behaviors or payloads for implementation to invent. The corrections below preserve the simplified architecture.

Review receipt: `scheduled-workflows-nine-table-2026-09-07-01/result-01`; mode `three-artifact-design`; target classification `general-domain`. This is an independent, candidate-only review—not implementation acceptance.

I read every line of the three separate artifacts:

| Target | Complete coverage | SHA-256 prefix |
|---|---:|---|
| [Requirements](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-requirements.md:1) | 97 lines | `d0a1fbaf3ed4` |
| [Specification](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:1) | 554 lines | `70a5b3459f6a2` |
| [Program Design](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:1) | 297 lines | `4c17e1a30f3e` |

All three hashes remained unchanged at completion. Your current instruction establishes authority for all S1–S42 rows and explicitly authorizes this fresh review. No prior review coverage was reused. The complete review ran directly; no agents or focused lanes were dispatched.

The model I reconstructed is:

```text
Instructions ──► Schedule ──► Run ──► exclusive native binding
                    │          └── current outcome and summary
                    └── conditional active/waiting pointers

Wake definition ──► firing + durable delivery ──► native receipt
                              │
Current records survive       └── previous attempts expire in events
```

The composition and wake-delivery diagrams agree with the written ownership boundaries. The missing scheduled-execution call path matters in F2 below.

1. **F1 — Imported continuity has no assigned current-state home. Important.**

   [Specification:215](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:215) requires later destination preparation to consume imported summary text. But [schedule storage:143](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:143) has no continuity field, and successful summaries exist only on local runs. The closed [CapturedRunInputs:360](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:360) carries a source-run ID but no imported text/provenance alternative.

   **Basis:** S3, S10, S22, S23, S42 require retained actual inputs and usable imported continuity.

   **Failure:** import without a native server, restart, then prepare. There is no specified current-record lookup for the imported summary or representation of its captured use. The raw package survives in an operation request, but the design never assigns receipts responsibility for selecting current continuity after overwrites.

   **Smallest correction:** assign imported continuity to the current schedule record; define its capture into run inputs, including text and external provenance. Keep nine tables.

   Route: `spec-design -> program-design`.

   **Confirm with:** import → restart → prepare → inspect captured inputs, including overwrite and event-pruning cases.

2. **F2 — Reusing normal auto delivery does not implement scheduled-start eligibility. Important.**

   [Specification:426](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:426) says an externally busy destination delays scheduled start. [Program Design:76](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:76) describes reuse of existing native submission, without defining the workflow-specific readiness path.

   Current [native_message_dispatch.rs:213](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/crates/communication-service/src/native_message_dispatch.rs:213) deliberately steers an already-active turn.

   **Basis:** S9, S15, S35 and the settled scheduled-start contract; S41 preserves ordinary messages.

   **Failure:** straightforward reuse submits scheduled instructions into work already known to be busy, instead of delaying. Another implementer must invent a separate readiness boundary.

   **Smallest correction:** specify the run-owner → generation-gated readiness read → eligible start → persisted turn receipt path, including when the timeout begins and how completion/cessation reaches summary handling and guarded release. Reuse native helpers; preserve normal auto semantics for messages and wakes.

   Route: `program-design`.

   **Confirm with:** a busy target causes no scheduled submission; the documented start race retains honest native evidence.

3. **F3 — Closed delivery evidence drops known partial effects. Important.**

   [Specification:170](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:170) requires `OutcomeUnknown` to carry `knownEffects`. The authoritative-looking closed [DeliveryEvidence:435](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:435) instead permits only attempt ID and explanation.

   Current [message_effect_state.rs:19](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/crates/communication-service/src/message_effect_state.rs:19) exposes separate resume/submission evidence and native correlation.

   **Basis:** S28, S31, S34, S35 require machine-readable partial effects and safe recovery.

   **Failure:** a confirmed resume followed by a lost submission response cannot retain its structured evidence through delivery inspection or archived attempt inspection. Clients must parse explanation text or lose information.

   **Smallest correction:** define one closed partial-effect payload, preserve relevant correlation/generation evidence, and use it consistently in current delivery evidence, attempts and errors.

   Route: `spec-design`.

   **Confirm with:** represent confirmed resume plus unknown submission without prose parsing, invented acceptance or automatic replay.

4. **F4 — Attempt and summary inspection cannot return promised information. Important.**

   [Specification:552](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:552) requires attempt pages to report their retained-history boundary and addresses successful continuity text by producing run ID. However:

   - [Page:300](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:300) contains only records and next cursor.
   - [SummaryInspection:452](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:452) exposes neither summary text nor its content provenance; `run/show` does not supply them either.

   **Basis:** S11, S28, S40, S42 require readable current summary state and honest bounded history.

   **Failure:** an initial history request after pruning cannot distinguish complete history from retained history. A caller holding the producing run ID cannot retrieve the promised summary through the listed run lookup methods.

   **Smallest correction:** give attempt-history responses explicit coverage metadata and expose retained summary text/provenance through the existing run inspection surface.

   Route: `spec-design`.

   **Confirm with:** initial and continued reads after pruning report coverage, retain the latest attempt exactly once, and retrieve successful summary content by run ID.

5. **F5 — Operation lookup has two incompatible result contracts. Important.**

   [Specification:312](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:312) defines a receipt-bearing result. [Specification:502](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:502) requires admitted, in-progress, uncertain, succeeded and failed variants, but does not provide their closed payload definitions.

   **Basis:** S18, S28, S34, S35 require a sufficient language-independent contract and truthful effects.

   **Failure:** after a lost fork response, `operation/show` must return uncertainty without a final receipt. A client generated from the earlier closed shape cannot represent that response.

   **Smallest correction:** replace the receipt-only declaration with an explicit tagged `OperationSnapshot`, defining required effects and method-specific success/failure payloads. Use it for both show and reconcile.

   Route: `spec-design`.

   **Confirm with:** every stored operation status maps to exactly one valid public variant; absent final evidence never becomes success.

6. **F6 — A paused one-shot can exhaust its timing without a defined wait outcome. Important.**

   [Specification:93](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:93) forbids replaying paused ticks. The [first-fire vocabulary:137](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-specification.md:137) has no finished-without-firing outcome.

   **Basis:** S26, S28, S32, S39.

   **Failure:** create a one-shot for 10:10 with no expiry, pause at 10:05, resume at 10:20. Its only occurrence must be skipped. A newly attached first-fire wait has neither a future firing nor a defined terminal error.

   **Smallest correction:** specify that resume finishes an exhausted one-shot without recording a firing, and define the corresponding typed first-fire error. Preserve historical-firing precedence.

   Route: `spec-design -> program-design`.

   **Confirm with:** this exact sequence terminates the new wait honestly; repeating reminders retain their original anchor and expiry.

7. **F7 — File-backed timeout configuration has no identified persistence/recovery owner. Important.**

   [Program Design:278](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/docs/specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-program-design.md:278) claims existing file-backed configuration ownership. The inspected [HostConfig:42](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/crates/codex-router-host/src/host_configuration.rs:42) is immutable launch configuration, constructed by [foreground_launch.rs:123](/Users/shravansunder/Documents/dev/project-dev/codex-router.feat-agent-messaging-v2/crates/codex-router-cli/src/host_command/foreground_launch.rs:123). These paths do not supply the claimed mutable automation configuration store.

   **Basis:** S19, S38 and the operation-correlated `automation/configure` contract.

   **Failure:** implementation must invent which file owns the defaults and how file replacement, configuration events and operation receipts recover after a crash. A committed receipt must not report values that were never durably applied.

   **Smallest correction:** identify the actual Host-composed configuration adapter, startup loading and serialized update/recovery ordering. Describe this as an explicit extension; no settings table or configuration engine is needed.

   Route: `program-design`.

   **Confirm with:** interrupted updates and operation replay agree with configuration loaded after restart; active attempts retain captured budgets.

All seven findings are source-backed review candidates inside the confirmed scope. Removing the underlying capabilities would violate governing requirements; none requires additional tables, distributed coordination or a manager. Parent disposition and remediation verification remain outstanding.

What held under independent inspection:

- Nine application tables, all 86 columns commented, zero triggers, and only the permitted enabled check.
- Exact composite FKs constrain current instruction revision, schedule active/waiting pointers, run binding ownership and wake pending delivery ownership. Native-address uniqueness is present. External references remain application-validated.
- Conditional admission and run-ID-guarded release are explicit; uncertainty and required summary retain the slot.
- Instruction revisions remain permanent; schedule/wake change IDs are distinct from instruction revision IDs.
- Current first-fire evidence survives event pruning. Attached waits preserve rapid pause/resume outcomes. Pause discards undispatched input; accepted input is not recalled.
- Import conflict, disabled import, instruction collision and local destination-preparation rules are substantially defined.
- Explicit replies remain C; event wakes and no-reply fallback remain deferred. Ordinary communication remains allowed.
- The proposed library/storage/service/Host separation is appropriate for a personal local system.

The simplification coverage disposition is:

| Requirement identities | Disposition and anchors |
|---|---|
| S1, S2 | Covered: Program Design 78–104; Specification R1. Exact Croner pin caveat below. |
| S3, S10, S22, S23 | Gap: F1 |
| S4, S20 | Covered: Specification R5 and portable-package conflict/change-ID rules |
| S5, S6, S7, S8 | Covered: Specification R2; Program Design 256–260 |
| S9, S15 | Gap: F2 |
| S11 | Gap: F4 |
| S12, S13, S14 | Covered: commented schema and separate persistence/composition boundaries |
| S16, S17 | Covered: Specification R3 and summary recovery; F1/F4 qualify continuity representation/inspection |
| S18, S34, S35 | Gap: F3–F5 |
| S19, S38 | Gap: F7 |
| S21 | Covered: verified discovery/default-endpoint contract and current discovery source |
| S24, S25 | Covered: R6 and wake operation inventory; F3 qualifies effect inspection |
| S26, S32, S39 | Gap: F6 |
| S27, S30 | Owner-authorized deferral: Specification R8 |
| S28, S31 | Gap: F3–F6 |
| S29, S33, S36 | Covered: reminder discard, cancellation and ordered first-fire rules |
| S37, S40 | Covered: five-field/timezone and retention contracts; exact Croner pin caveat below |
| S41 | Covered: Specification 426; Program Design 258 |
| S42 | Gap: F1/F4; nine-table structure itself verified |

Evidence limits: eight in-memory SQLite schema/constraint probes passed, exit `0`, including cross-owner FK rejection, duplicate native-address rejection and sequential conditional claim/stale-release behavior. These do **not** prove concurrent workers, SQLx integration or crash recovery. No servers, model tests, secrets inspection or filesystem edits occurred.

Current Rust inspection covered CLI message capture, discovery, Control client/dispatch, native effects and interruption, Host composition/configuration, lifecycle observation and native schema admission. The local upstream Codex checkout matched the cited commit; fork, history-pagination and turn-notification types were inspected. Exact Croner **4.0.0** source verification remained unavailable; the accessible [parser source](https://docs.rs/croner/latest/src/croner/parser.rs.html) supports the named API family but does not establish that pin. No version defect or runtime conformance is inferred.

Recommended next owner: `spec-design`, carrying F1 and F3–F6 against these unchanged artifacts, followed by the identified Program Design corrections. Planning remains premature until those contracts and integration paths are concrete. No remediation or acceptance is claimed.