# Typed provider callbacks

Expected: genuine Question/Approval emission → actual Session test decision → provider callback/tool/turn settlement → typed SQLite terminal/revisions → identical owning-Host reopen. Automated test decisions are not Human decisions. The Claude journey was observed on Sunclaw through successful owning reopen. The final portable parameterized cold entry and Cursor route remain draft until driven.

The real Claude Question path is `AskUserQuestion` → installed ACP adapter `handleAskUserQuestion` → `elicitation/create` → `ProviderFormElicitationHandler` → `HostInteractionPort` → typed broker. Router advertises form elicitation in `provider_initialize_request.rs`. The adapter adds an optional `question_0_custom` text field next to the single-choice field; answering an offered option may omit that text field.

The broker validates and sends Question responses before persisting them; Approval persists before sending the selected option. A public acknowledgement or terminal SQLite row alone does not prove callback consumption. Sources: `crates/acp-client-runtime/src/agent_session_client/{provider_form_elicitation,external_approval_dispatch}.rs`, `crates/codex-router-host/src/acp_interaction_port.rs`, and `crates/collaboration-service/src/interaction_broker/typed_interactions.rs`.

## Bind a real Approver

Use a genuine Router-owned provider Session on this same debug Host as the test Approver. Retain its returned exact SessionRef and current binding. Configure a harmless no-tools response to notices through supported provider operations. The test driver acts explicitly as that actual Session. Do not relabel a production native Session ID with this debug service: the first such route produced a real Question immediately cancelled `approverUnreachable`. Native session inspection does not establish notice reachability.

Typed notices are stored pushes delivered through `typed_interaction_notice.rs`; `session_message_route_composition.rs` installs the provider ACP route. `ProviderAcpDeliveryRoute` claims only actual provider session records, checks live ownership/settings, and submits or steers the prepared notice through its real runtime. Stop at unsupported/ambiguous routing before another model prompt. No product repair or new transport follows from this recipe.

## One fresh requester journey

Read `JourneyPlan` in `scripts/verification/provider_session_control.py`: qualified `cli`, fresh `evidence` subdirectory, explicit `service_directory`, actual `caller_session_id`, explicit `endpoint_id`, and `settings` mapping. `approver` is the returned exact real provider SessionRef for the requester. `tools` holds qualified `question_tool`, `execution_tool`, `question_method`, `evidence_path` and SHA256. There are no implicit Claude tool/model defaults. The supported question methods are existing `elicitation/create` and `cursor/ask_question`; protocol names are not guesses at provider tool names. Provenance hashes do not themselves prove tool support; the operator must inspect the actual source/catalog witness. Set `JOURNEY_PLAN` to this private JSON file. Its evidence subdirectory sits beside lifecycle `workspace`.

First create the Approver, qualify its returned actual catalog/settings and initialize it with the `initialize-approver` action: one harmless no-tools instruction. Confirm exact completion, idle state and no tool/interaction events. Then create the distinct requester using that real returned SessionRef as `approver`. The requester driver re-inspects both actual Sessions before its one model input. Never remap a native production Session ID into this debug service.

```sh
uv run --offline --no-project --with pydantic --python 3.13 python -m scripts.verification.interaction_journey create "$JOURNEY_PLAN"
```

One create with explicit selected settings and write-restricted access; verify its actual endpoint generation, returned catalog, Router elicitation-route capability, operation and auth status. `settings: {}` is metadata-only and blocks model input until settings are qualified. CLI validates requested advertised choices. The Host supplies its Router-owned Claude route/token without exporting credentials. Stop unsupported settings or Question capability before model input. A create decoding failure authorizes only `inspect` of the retained operation, never another create.

```sh
uv run --offline --no-project --with pydantic --python 3.13 python -m scripts.verification.interaction_journey drive "$JOURNEY_PLAN"
```

One prompt requests actual AskUserQuestion first, exactly Blue/Green. The driver answers the real Blue option ID under its declared Session actor. It then validates the actual Execute request's reviewed exact private marker command and selects only an offered allow/once option without a persistent target. Unexpected actions/options/order stop without a grant. No retry, fallback model, permission-as-Question substitution or persistent/session grant.

The independent observer understands JSON and SSE, retaining typed interactions/tool/turn facts and omitting thoughts/provider bodies. Await its original bounded call; an unrelated short future-wait deadline must not truncate it. Read-only `observe` can recover retained evidence after a harness error; `verify-retained` applies corrected oracles and reads current scoped rows without repeating the prompt. It never replays input. The provider protocol unconditionally reports `continuationGap: true`; retain it without claiming uninterrupted global continuity. Reject resync/oversize markers, wrong target/binding, duplicate/reordered sequences or missing scoped facts. Ordinary prompts emit `TurnStarted(inputId)`; `InputAccepted` is emitted for steering. Public operation `inputId` is optional. Record operation/input/turn as distinct identities; when its direct public join is absent, attribute only the exact fresh requester, single actual started/ended turn, sole retained prompt intent and exact typed callback/tool facts. A success requires terminal/applied/confirmed operation, both resolutions, exactly the intended completed tools, ended turn, exact marker bytes and answered/decided SQLite rows. These counts do not establish other source/full-body/order obligations.

After all callers and the turn settle, `close` closes only this owned Session and waits for its operation. Then follow the main skill's Host A→B procedure and exact public/SQLite readback. Cancelled-history reopen remains negative evidence. Return failed route/IDs to the Implementer/Lead; do not rewrite expectations or repair product code.
