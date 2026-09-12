# Testing scheduled automation locally

Use the existing `codex-router-debug` profile and fresh test threads. The opt-in acceptance Host selects `gpt-5.6-luna` in memory, keeps normal Codex home, and puts its sockets, automation database and workspace in a new private directory under `/tmp`. It disables home hooks only in its owned test app-server so they cannot inject extra work. It uses the existing debug router credentials. It never edits the home profile or replaces the production router.

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

### Restart the Host for board persistence proof

Build the board test binary, start a fresh debug Host as above, and run phase one:

```sh
cargo test -p agent-sessions --test board_debug_acceptance --no-run
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-sessions --test board_debug_acceptance \
  two_luna_agents_exchange_a_verified_finding_through_the_board_cli \
  -- --ignored --exact --nocapture
```

Phase one uses two fresh Luna sessions and exercises references across two
projects, watches, inbox acknowledgement, history, and archived/resolved write
rejection. It saves private state for the persistence check.

The project-board acceptance journey may reopen the same isolated service data
after the original foreground Host has exited. Record the original `hostPid`
from `debug-host-context.json`, stop that owned Host with Ctrl-C, and relaunch
the same binary with the explicit resume option:

```sh
./target/debug/examples/automation-debug-host \
  --resume-run-directory "$proof_root" \
  --router-binary "$PWD/target/debug/codex-router" \
  --port 18787
```

Resume accepts only an existing owner-private direct child of `/tmp` whose
context marker still identifies the same `codex-router-debug` profile, Luna
model, port, service directory and workspace. It refuses symlinks, a live old
Host PID, a still-published service, or a mismatched marker. It preserves the
service databases and replaces the context marker atomically with the new Host
PID. Wait for `host status` and endpoint discovery to report readiness again,
then verify the new PID differs from the recorded PID before reading the board.
Do not use resume after an indeterminate stop or with a directory from another
test run.

After readiness is confirmed, run phase two without starting additional models:

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-sessions --test board_debug_acceptance \
  board_state_survives_owned_debug_host_restart \
  -- --ignored --exact --nocapture
```

It verifies persisted projects, references, archive state, watches and read
acknowledgements through the CLI after confirming the Host PID changed. Stop the
owned resumed Host with Ctrl-C after this check.

## Run the schedule and recovery scenarios

Start a new debug Host with a previously unused `proof_root` for **each** command below, using the same launch and readiness checks above. Stop the previous test Host first. Do not run the whole ignored test binary against one root: scenarios change test-owned settings or restart its backend, and some diagnostic tests require their own recorded identities.

Fresh scheduled execution with continuity:

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-sessions --test debug_luna_acceptance \
  fresh_scheduled_run_uses_previous_luna_summary \
  -- --ignored --exact --nocapture
```

The first worker produces a unique token. A separate Luna summary carries it into a different native execution thread after the current instructions have been changed to omit the token. The test requires actual worker output and holds the Run's execution slot through required summarization.

Busy-thread behavior and owned app-server replacement:

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-sessions --test debug_luna_acceptance \
  scheduled_work_waits_while_ordinary_messages_steer \
  -- --ignored --exact --nocapture
```

The schedule waits while ordinary input steers the busy thread. The driver interrupts only its exact original test turn, confirms cessation, then observes a distinct scheduled turn. It uses the existing Host restart command with verified test ownership to replace that app-server. It checks the new native generation, unchanged completed Run evidence, and queue rejection for the now-unloaded saved thread. This establishes app-server replacement behavior, not automatic TUI reconnection or a full Host-process crash.

Worker timeout, summary retry, import and durable delivery:

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-sessions --test debug_luna_acceptance \
  summary_recovery_and_durable_delivery_preserve_original_work \
  -- --ignored --exact --nocapture
```

The scheduler interrupts a worker using its captured two-second override. The driver separately interrupts an exact test summary; an explicit retry preserves the worker result and Run identity. The exported real summary is imported disabled into a separate test-owned automation store without creating synthetic Runs.

For delivery recovery, this scenario uses production service code over paired Unix streams with the real debug Codex backend and its generated validators. A controlled backend-admission gate proves first firing before acceptance, known non-submission, and later acceptance under the same delivery ID with exactly one correlated native input. It also verifies the typed pause-before-first-fire error. This controlled transport does not prove Host discovery or a whole-process restart; those boundaries have separate coverage.

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
