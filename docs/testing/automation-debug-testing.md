# Testing scheduled automation locally

Use the existing `codex-router-debug` profile and fresh test threads. The opt-in acceptance Host selects `gpt-5.6-luna` in memory, keeps normal Codex home, and puts its sockets, automation database and workspace in a new private directory under `/tmp`. It uses the existing debug router credentials. It never edits the home profile or replaces the production router.

## Build once

Run from the repository root:

```sh
cargo build -p codex-router-cli --bin codex-router
cargo build -p codex-router-host --example automation-debug-host
cargo test -p agent-sessions --test debug_luna_acceptance --no-run
```

The last command also builds the CLI used by the agent. The live test is ignored by default and requires an explicit invocation.

## Start the debug Host

In one terminal, choose a previously unused name:

```sh
proof_root=/tmp/luna-proof-my-check
./target/debug/examples/automation-debug-host \
  --run-directory "$proof_root" \
  --router-binary "$PWD/target/debug/codex-router" \
  --port 18787
```

Keep this foreground process running. An existing directory, occupied port, production port, alternate Codex home or invalid debug provider is rejected. The existing debug profile must point at the chosen debug port.

`debugHostPrepared` identifies the paths and Luna selection. It precedes startup readiness. In a second terminal, using the same `proof_root` value, wait for `agent-communication/service.json` and inspect readiness:

```sh
proof_root=/tmp/luna-proof-my-check
./target/debug/codex-router host status \
  --router-root "$proof_root" --port 18787 --require-debug-isolation
./target/debug/agent-sessions endpoints list \
  --service-directory "$proof_root/agent-communication" --json
```

The Host should report `OwnedReachable` and `NativeReady`; endpoint discovery must include a native generation and schema digest. `LocalReadyRemoteDegraded` is expected with Remote Control disabled for this local debug setup. A prepared marker alone does not establish readiness.

The outer test process needs permission to use the debug Unix sockets and normal Codex state. The Luna threads themselves use their own `workspace-write` sandbox and `approvalPolicy=never`; the acceptance test does not remove that sandbox.

## Run the agent-to-agent acceptance test

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-sessions --test debug_luna_acceptance \
  luna_agents_arrange_wake_and_reply_through_the_real_cli \
  -- --ignored --exact --nocapture
```

This test creates two new Luna threads. A calls the actual CLI to arrange a timed message to B. B calls `message send` to reply explicitly. The test checks the durable firing, native delivery acceptance, B's successful CLI receipt, the incoming message in A's thread, and A's acknowledgement. An agent's completion text alone is insufficient.

A separate opt-in OS test verifies the socket permission boundary without a
model or app-server:

```sh
cargo test -p agent-sessions --test native_sandbox_access \
  codex_sandbox_requires_exact_control_socket_permission -- --ignored --exact --nocapture
```

The test uses each root once. After a failure, inspect its private `proof-events.jsonl`; do not rerun against those same threads or silently resubmit unknown work. A newly created thread may reject history before its first input and briefly after acceptance while native metadata becomes available. The helper records the exact known readiness errors and keeps observing within its deadline. It still requires actual turn history and receipts to pass; it never resends input to make history appear.

Stop the foreground debug Host with Ctrl-C when finished. It shuts down its retained children. The private test artifacts remain for inspection, and Codex retains its ordinary session records. No directory deletion or production restart is part of this procedure.

## Inspect durable outcomes

The following commands use the same Rust SDK as applications. Replace each identity with the UUID from its creation or listing response:

```sh
./target/debug/agent-sessions operation show --operation-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-sessions operation reconcile --operation-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-sessions delivery show --delivery-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-sessions delivery reconcile --delivery-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-sessions run show --run-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-sessions run reconcile --run-id UUID --service-directory "$proof_root/agent-communication" --json
```

Use these while the owning Host is running. Reconciliation observes native evidence and may persist a confirmed outcome; it never resends input, starts a native thread or interrupts work. Configuration reconciliation can recover the original intended local settings-file update. Reconciliation of an older completed operation returns its original outcome without restoring old settings.

| Observation | What it establishes |
| --- | --- |
| `wake send` creation result | The timed message was durably arranged. |
| `--wait-until-first-fire` | The first firing was recorded; native acceptance is separate. |
| Delivery accepted | Codex accepted the input; agent completion or a reply is separate. |
| Delivery uncertain | The original attempt has unresolved effects; do not blindly resend. |
| Run stopping | An interruption was requested; cessation is not yet established. |
| Run finished | The recorded native work and required summary handling reached their terminal states; it does not certify the objective was achieved. |

The observer uses `thread/read` with `includeTurns=true` and verifies the returned
thread and exact turn identity. Codex 0.153.4 can temporarily reject this read with
`list_turns is not supported yet` before its new-thread metadata exists. Native history remains subject to the native
carrier's frame bound; oversized or missing required context fails explicitly.

An exact queue entry can recover a lost queue receipt. Queue absence cannot establish non-submission because the entry may already have been consumed. Reconciliation retains uncertainty when original operation/resume evidence or native identities are insufficient.

## Automated gates

The permanent SQLite, filesystem, CLI and scripted-native tests exercise failure/recovery cases without paid model calls. They remain distinct from the opt-in live journey above.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
cargo audit
python3 -m unittest scripts.tests.test_update_homebrew_formula -v
```

CI also builds the router with all features, checks an isolated Cargo installation, and runs the quota-reset PTY harness. Preserve those gates when preparing the PR.

Source: [debug Host launcher](../../crates/codex-router-host/examples/automation-debug-host.rs), [live acceptance test](../../crates/agent-sessions/tests/debug_luna_acceptance.rs), [scheduled workflow requirements](../specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-requirements.md).
