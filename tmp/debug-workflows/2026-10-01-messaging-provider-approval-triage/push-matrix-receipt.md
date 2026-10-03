# Push-format acceptance matrix receipt

Status: INCOMPLETE. Model-free recipient observation is green. The first documented-model run failed during target materialization; the post-login retry materialized all three targets but stopped at the owned Host restart command. No S3/S5/S6 push cell has passed.

Source/gate anchor: e17e5c5f09e19e5dd0bfa0c2095e93d7ef8ade8a plus the seven owned matrix paths listed below. The commit hash is appended after integration.

## Live results

Model-free run `01a0f9a8-b170-7882-9225-a89d48c33ed9`: `push_delivery_matrix_covers_codex_and_claude_peer`, 1 passed / 0 failed, exit 0, 6.90 s. Fresh private root `/tmp/push-matrix-free-20261001-60aa64df`; foreground debug CLI Host; actual Codex app-server 0.159.2 and Claude fixture user frames. No completed model turn is claimed.

| Cell | Result | Observed surface |
| --- | --- | --- |
| S1 | PASS | Real Codex native userMessage, exact notice equality |
| S2 | PASS | Claude fixture user frame, 100-scalar preview, +1240, CLI show full body |
| S3 | FAIL preparation; actual held send not attempted | Real documented model Host; materialization deadline |
| S4 | PASS | Two Claude frames and real Codex reply input; explicit first PushId targeting, missing reference rejected |
| S5 | UNATTEMPTED | Blocked by model target preparation |
| S6 | UNATTEMPTED | Blocked by model target preparation |
| S7 | PASS | 31-day record pruned, recipient CLI show rejected notFound, exit 4 |
| S8 | PASS | Third-party CLI show rejected notPermitted, exit 4 |
| S9 | PASS | Claude frame visibly Owner (unverified) |
| S10 | PASS for observed cells | Actual inputs equal one rendered notice line; model cells remain unverified |

## Exact observed first lines

### S1

```text
✉️ 🤖 codex-local/01a0f9a8 @Sunbook-Pro-M4.local → you · "S1 is green" · router://1a1e5195-6b34-4b71-b42d-dac3c5b12ff2/push/01a0f9a8-b6e5-7310-8aea-626dcb3cd50f
```

### S2

```text
✉️ 🤖 codex-local/01a0f9a8 @Sunbook-Pro-M4.local → you · "PUSH_DELIVERY_MATRIX_long-receipt_01a0f9a8-b7fd-7580-b908-52e61e8981e6 RRRRRRRRRRRRRRRRRRRRRRRRRRRRR" (+1240) · router://1a1e5195-6b34-4b71-b42d-dac3c5b12ff2/push/01a0f9a8-b803-7540-b668-7f3be45fa6c0
```

### S4

```text
✉️ 🤖 codex-local/01a0f9a8 @Sunbook-Pro-M4.local → you · "Message from A" · router://1a1e5195-6b34-4b71-b42d-dac3c5b12ff2/push/01a0f9a8-b85d-7a93-a07b-bdf8a65d1970
✉️ 🤖 codex-local/01a0f9a8 @Sunbook-Pro-M4.local → you · "Message from B" · router://1a1e5195-6b34-4b71-b42d-dac3c5b12ff2/push/01a0f9a8-b868-7323-853f-0fd08df35ded
✉️ ✳️ claude-local/fixture- @Sunbook-Pro-M4.local → you · "done" · router://1a1e5195-6b34-4b71-b42d-dac3c5b12ff2/push/01a0f9a8-b874-74a3-ac5e-c35d099fe631
```

### S8

No recipient push is emitted by this negative cell. Exact recipient CLI first output line:

```json
{"kind":"error","error":{"kind":"rejected","serviceKind":"notPermitted","stage":"inspect","effect":"none","message":"Not permitted to read this push","code":-32050,"data":{"kind":"notPermitted","stage":"inspect","message":"Not permitted to read this push"}}}
```

### S9

```text
🧑 Owner (unverified) @Sunbook-Pro-M4.local → you · "Owner's compact note" · router://1a1e5195-6b34-4b71-b42d-dac3c5b12ff2/push/01a0f9a8-babc-7dc1-9eb7-2ac344955e49
```

### S7

No recipient push is emitted by this negative cell. Exact recipient CLI first output line:

```json
{"kind":"error","error":{"kind":"rejected","serviceKind":"notFound","stage":"inspect","effect":"none","message":"not found (expired after 30 days, or never existed)","code":-32050,"data":{"kind":"notFound","stage":"inspect","message":"not found (expired after 30 days, or never existed)"}}}
```

### S10

```text
✉️ 🤖 codex-local/01a0f9a8 @Sunbook-Pro-M4.local → you · "S1 is green" · router://1a1e5195-6b34-4b71-b42d-dac3c5b12ff2/push/01a0f9a8-b6e5-7310-8aea-626dcb3cd50f
```

### Scripted ACP recipient column

Run identity: fresh root `push-matrix-acp-20261001-8d935a47` (this test did not emit a separate run UUID); `push_delivery_matrix_reaches_scripted_acp_target`, 1 passed / 0 failed, exit 0, 11.39 s. Observed actual scripted provider session/prompt; this is a fixture column, not evidence of a real provider model completion.

```text
✉️ 🤖 codex-local/01a0f983 @Sunbook-Pro-M4.local → you · "PUSH_DELIVERY_MATRIX_acp-target_01a0f983-4289-77c2-a5f2-ed76c93c2dad" · router://d5ea1041-3774-40e5-9912-6a157617e224/push/01a0f983-43af-78d2-8a92-da3831ac0717
```

## Documented model route blocker

Run `01a0f995-b86c-76f2-bdb1-072109f1e2ca`, root `/tmp/push-held-model-20261001-4183a8c2`; `push_delivery_matrix_holds_codex_recipients` failed, exit 101, 128.16 s. S3 recipient materialization timed out. S3 held DM was never sent; S5/S6 were not attempted. No push first line exists for these cells.

Native observations: responseStreamDisconnected, connectionReset, websocket retries. Private router audit: 176 events; 7 attempted accounts rejected as provider_credential; local auth valid; models 70 and response websocket 84 credential rejections; 22 selection rejections; 0 committed responses. Existing debug-state aggregate: 8 enabled OpenAI accounts, 7 reauth_required/provider_outcome_ambiguous, 1 retrying/local_persistence with 3 consecutive failures; models/responses quota refresh each has 8 auth_error records. Cached quota headroom is not uniformly zero, so quota exhaustion is not established. Account identifiers, emails, tokens, hashes and credentials are omitted.

Owner response after that run: the owner reauthorized an existing DEBUG account. The first run did not reach Host restart; the post-login retry is detailed below. No credentials were copied, accounts switched, production fallbacks used, or production processes replaced. The same existing acceptance-Host restart guard remains the only restart path invoked.

## Post-login retry on 2026-10-02

After the Lead confirmed the `debug-primary` login was complete, the exact acceptance sequence ran against a fresh private run root `/tmp/push-held-model-20261002-dec59eb0`, using the foreground `automation-debug-host` at debug port 18787. `codex-router host status --router-root <run-root> --port 18787 --require-debug-isolation` exited 0 and reported the Router and app-server ready.

`CODEX_AUTOMATION_PROOF_ROOT=<run-root> cargo test -p agent-collaboration --test push_delivery_matrix push_delivery_matrix_holds_codex_recipients -- --ignored --exact --nocapture` exited 101 (0 passed / 1 failed). The private proof events record completed materialization for the S3, S5, and S6 target conversations, then `ownedBackendRestartRequested` and `ownedBackendRestartResult` with CLI exit code 2 and `succeeded: false`. The restart status output was `host replacement command is unavailable`. The restart occurs before the matrix push cells, so S3 did not send its held DM, S5 did not send its subscription batch, and S6 did not send its wake or scheduled-run push. No exact S3/S5/S6 push first lines exist for this run.

Redacted Router audit aggregate for this run: 1 `provider_credential` event (2 total `credential` mentions), 0 `selection_rejected`, 0 `committed_response`, 0 `quota`, 0 `unauthorized`, and 0 `responseStreamDisconnected`, `connectionReset`, or `websocket` matches. The three model target materialization turns completed; the available aggregate does not attribute the credential rejection to the freshly logged-in `debug-primary`. No account identifiers, labels, hashes, emails, or credential values are recorded here. Per Lead instruction, the test stopped at this failure and did not wait for the second account re-login.

The foreground debug Host exited 0 on Ctrl-C. A follow-up status check against the same run root reported `no shared Codex host is running` (exit 2). No production Router/Host was accessed or restarted.

## Source and quality evidence

- `cargo test -p agent-collaboration --test push_delivery_matrix`: 3 passed / 5 ignored, exit 0; the three defaults are imported native-diagnostic tests, not held-cell live proof.
- `cargo clippy -p agent-collaboration --all-targets -- -D warnings`: exit 0 on the final matrix source (1.54 s).
- `cargo check --workspace`: exit 0 (13.54 s).
- `rustfmt --edition 2024 --check` for all seven owned paths: exit 0 after final source review.
- `git diff --check -- <seven owned paths>`: exit 0.
- Owner Codex/Claude settings hash sentinel verified at entry, per observed cell, and exit; model test verifies it on early failure too.

Owned paths:

- `crates/agent-collaboration/tests/push_delivery_matrix.rs`
- `crates/agent-collaboration/tests/delivery_matrix/held_cells.rs`
- `crates/agent-collaboration/tests/delivery_matrix/held_subscription_producer.rs`
- `crates/agent-collaboration/tests/delivery_matrix/push_matrix_driver.rs`
- `crates/agent-collaboration/tests/delivery_matrix/subscription_notice_observer.rs`
- `crates/agent-collaboration/tests/delivery_matrix/subscription.rs`
- `crates/agent-collaboration/tests/delivery_matrix/support.rs`

S3 verifies the stored Held DM, explicit load, same PushId and exact recipient input. S5 verifies two exact Board roots Wakeable/pending/held before load, one native notice after load, two stored activity ranges and held-since facts. Those assertions are compiled but remain live-unverified. Helpers are split by CLI/native driving, held producer, and notice observation. No production source behavior changed.

## Runtime cleanup and remaining scope

All owned foreground Hosts exited via Ctrl-C. Model-free Cargo session 59377 exited 0. Ports 43127/43128/18787 had no listeners at the final check. The abandoned foreground-CLI restart guard is absent; existing automation_live_support/debug_backend_restart.rs is unchanged. 29b was notified that no matrix test, build, gate or Host still depends on shared target artifacts; authorized dev-profile cleanup can proceed.

Trace: service `0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89`, project `01a09c8d-bfa1-7eb2-8432-595da973724f`, board `01a09c8f-fe30-7480-a9a0-cbd27a6f76e5`, topic `01a0eae4-75b8-7ff3-a407-a599e8871fc9`, execution root `01a0f6f3-2e84-7c20-bac0-f5c5fcfd3816`. Outer work remains unresolved.

No full #112 acceptance, push, release or production restart claim. Next useful action is the owner/Lead debug-credential disposition and a fresh S3/S5/S6 live run.

Commit checkpoint: `d07512b2` contains exactly the seven owned matrix paths. The subsequent integrator commit is `924de0e4`; no source changes to our matrix paths occurred between proof and commit. Live acceptance remains incomplete for S3/S5/S6.
