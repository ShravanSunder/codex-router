# Testing scheduled automation locally

Use the existing `codex-router-debug` profile and fresh test threads. The opt-in acceptance Host selects `gpt-5.6-luna` in memory, keeps normal Codex home, and puts its sockets, automation database and workspace in a new private directory under `/tmp`. It disables home hooks only in its owned test app-server so they cannot inject extra work. It uses the existing debug router credentials. It never edits the home profile or replaces the production router.

## Build once

Run from the repository root:

```sh
cargo build -p codex-router-cli --bin codex-router
cargo build -p codex-router-host --example automation-debug-host
cargo test -p agent-collaboration --test debug_luna_acceptance --no-run
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
./target/debug/agent-collaboration endpoints list \
  --service-directory "$proof_root/agent-communication" --json
```

The Host should report `OwnedReachable` and `NativeReady`; endpoint discovery must include a native generation and schema digest. `LocalReadyRemoteDegraded` is expected with Remote Control disabled for this local debug setup. A prepared marker alone does not establish readiness.

The outer test process needs permission to use the debug Unix sockets and normal Codex state. The Luna threads themselves use their own `workspace-write` sandbox and `approvalPolicy=never`; the acceptance test does not remove that sandbox.

## Run the agent-to-agent acceptance test

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-collaboration --test debug_luna_acceptance \
  luna_agents_arrange_wake_and_reply_through_the_real_cli \
  -- --ignored --exact --nocapture
```

This test creates two new Luna threads. A calls the actual CLI to arrange a timed message to B. B calls `message send` to reply explicitly. The test checks the durable firing, native delivery acceptance, B's successful CLI receipt, the incoming message in A's thread, and A's acknowledgement. An agent's completion text alone is insufficient.

A separate opt-in OS test verifies the socket permission boundary without a
model or app-server:

```sh
cargo test -p agent-collaboration --test native_sandbox_access \
  codex_sandbox_requires_exact_control_socket_permission -- --ignored --exact --nocapture
```

The test uses each root once. After a failure, inspect its private `proof-events.jsonl`; do not rerun against those same threads or silently resubmit unknown work. A newly created thread may reject history before its first input and briefly after acceptance while native metadata becomes available. The helper records the exact known readiness errors and keeps observing within its deadline. It still requires actual turn history and receipts to pass; it never resends input to make history appear.

Stop the foreground debug Host with Ctrl-C when finished. It shuts down its retained children. The private test artifacts remain for inspection, and Codex retains its ordinary session records. No directory deletion or production restart is part of this procedure.

## Run the recipient-observed delivery matrix

This matrix uses the foreground CLI Host with an isolated `HOME`, `CODEX_HOME`, Router root, sockets, and workspace. It never reads the owner's Codex or Claude session registry. The setup test creates a fresh owner-private direct child of `/tmp`, a non-secret debug profile, a symlink to the installed Codex executable, and `providers.json` pointing at the repository's scripted Cursor ACP fixture. It copies no credentials. The CLI Host reads `providers.json` through its normal provider configuration path. Announce the shared 43127 port before starting; stop only this foreground Host with Ctrl-C and release both ports when done.

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-collaboration --test push_delivery_matrix \
  prepare_delivery_matrix_provider_fixture \
  -- --ignored --exact --nocapture
```

Start the Host in a separate foreground terminal after setup:

```sh
env HOME="$proof_root/home" CODEX_HOME="$proof_root/codex-home" \
  CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET="$proof_root/native-socket/app-server.sock" \
  ./target/debug/codex-router host --router-root "$proof_root" \
  --port 43127 --mcp-bind 127.0.0.1:43128 --require-debug-isolation
```

Require `codex-router host status --router-root "$proof_root" --port 43127 --require-debug-isolation` to report router and app-server ready, and confirm `cursor-local` advertises the scripted fixture. If the isolated home or private Router lacks model access, report that state without copying account data or switching to the owner Codex home.

```sh
matrix_cargo_home="${CARGO_HOME:-$HOME/.cargo}"
matrix_rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
env HOME="$proof_root/home" CODEX_HOME="$proof_root/codex-home" \
  CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  CARGO_HOME="$matrix_cargo_home" RUSTUP_HOME="$matrix_rustup_home" \
  cargo test -p agent-collaboration --test push_delivery_matrix \
  push_delivery_matrix_covers_codex_and_claude_peer \
  -- --ignored --exact --nocapture
```

The suite exercises CLI and MCP messages, fired wakes, scheduled runs, Board Thread Listen pushes, and approval notices. Its Codex ACP load-route cell runs CLI `conversation create` then `conversation prompt --to` on separate connections with provider routes present; it rejects any failure before prompt dispatch. The isolated home has no model authentication, so that cell proves load routing, not prompt completion. The approval requester is the scripted Cursor ACP fixture; native Codex approvals under the owner's `auto_review` reviewer do not reach Router. It counts a Codex message cell only after the exact input appears in `thread/read(includeTurns=true)`, and a Claude peer cell only after its fixture socket reads the user frame. Approval cells also require a pending broker record before the approver decides. The suite compares owner `~/.codex/config.toml` and `~/.claude/settings.json` hashes at entry, after each cell, and at exit; a change fails the run and must be reported without restoration. A five-minute Board Listen debounce makes the complete run longer than a typical smoke test.

For a quick load-route regression check, use a separate freshly prepared matrix root and run the ignored `codex_acp_cli_create_then_prompt_load_route` test with the same environment and foreground Host. Its CLI create and prompt calls use separate ACP connections, so a missing `_meta.router.sessionRef` fails at load before the model is invoked.

For the Router-hosted ACP target column, use a **different fresh root** and run `prepare_delivery_matrix_acp_target_fixture` instead of `prepare_delivery_matrix_provider_fixture`. Start the same isolated CLI Host as above, then run `push_delivery_matrix_reaches_scripted_acp_target` from the `push_delivery_matrix` test target with the same private `HOME`, `CODEX_HOME`, `CODEX_AUTOMATION_PROOF_ROOT`, `CARGO_HOME`, and `RUSTUP_HOME` environment. This variant starts two scripted providers: Cursor receives the six producer inputs, while Claude requests permission from Cursor as Approver. The fixture records each actual `session/prompt` frame and requires each marker in exactly one prompt. The test verifies the approval is still pending after its notice reaches Cursor, then decides deny-once so the requester can settle. The additional busy-target cell requires an `auto` CLI send to return `queued`, then observes its one prompt after the held turn settles, matching the manual's deferred-input contract. Keep the owner settings hash sentinel and stop the foreground Host after the suite.

The ACP column does not attest the Host's `HOME` or `CODEX_HOME`. It relies on `--require-debug-isolation` (launchctl-free), the documented launch environment, and the owner settings hash sentinel; the Host does not yet report its PID or resolved homes (product gap logged).

A Codex thread active in another app-server or desktop client remains a separate target row needing recipient-observed proof.

The materialized existing Codex target is a pending matrix cell in this isolated run: the private Codex home has no model authentication to finish an initial turn and return the thread to idle. A default-run fake app-server integration test covers its declared cwd and scheduled turn/start. Recipient-observed live proof remains for the post-release real-session run.

After the owner replaces production with a release containing this suite's fixes, repeat one documented pass against real sessions: read the Codex recipient's exact input through `thread/read`, and obtain an explicit receipt confirmation from the Claude Code recipient. Keep that live result separate from the isolated fixture matrix.

### Restart the Host for board persistence proof

Build the board test binary, start a fresh debug Host as above, and run phase one:

```sh
cargo test -p agent-collaboration --test board_debug_acceptance --no-run
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-collaboration --test board_debug_acceptance \
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
  cargo test -p agent-collaboration --test board_debug_acceptance \
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
  cargo test -p agent-collaboration --test debug_luna_acceptance \
  fresh_scheduled_run_uses_previous_luna_summary \
  -- --ignored --exact --nocapture
```

The first worker produces a unique token. A separate Luna summary carries it into a different native execution thread after the current instructions have been changed to omit the token. The test requires actual worker output and holds the Run's execution slot through required summarization.

Busy-thread behavior and owned app-server replacement:

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-collaboration --test debug_luna_acceptance \
  scheduled_work_waits_while_ordinary_messages_steer \
  -- --ignored --exact --nocapture
```

The schedule waits while ordinary input steers the busy thread. The driver interrupts only its exact original test turn, confirms cessation, then observes a distinct scheduled turn. It uses the existing Host restart command with verified test ownership to replace that app-server. It checks the new native generation, unchanged completed Run evidence, and queue rejection for the now-unloaded saved thread. This establishes app-server replacement behavior, not automatic TUI reconnection or a full Host-process crash.

Worker timeout, summary retry, import and durable delivery:

```sh
CODEX_AUTOMATION_PROOF_ROOT="$proof_root" \
  cargo test -p agent-collaboration --test debug_luna_acceptance \
  summary_recovery_and_durable_delivery_preserve_original_work \
  -- --ignored --exact --nocapture
```

The scheduler interrupts a worker using its captured two-second override. The driver separately interrupts an exact test summary; an explicit retry preserves the worker result and Run identity. The exported real summary is imported disabled into a separate test-owned automation store without creating synthetic Runs.

For delivery recovery, this scenario uses production service code over paired Unix streams with the real debug Codex backend and its generated validators. A controlled backend-admission gate proves first firing before acceptance, known non-submission, and later acceptance under the same delivery ID with exactly one correlated native input. It also verifies the typed pause-before-first-fire error. This controlled transport does not prove Host discovery or a whole-process restart; those boundaries have separate coverage.

## Inspect durable outcomes

The following commands use the same Rust SDK as applications. Replace each identity with the UUID from its creation or listing response:

```sh
./target/debug/agent-collaboration operation show --operation-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-collaboration operation reconcile --operation-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-collaboration delivery show --delivery-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-collaboration delivery reconcile --delivery-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-collaboration run show --run-id UUID --service-directory "$proof_root/agent-communication" --json
./target/debug/agent-collaboration run reconcile --run-id UUID --service-directory "$proof_root/agent-communication" --json
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

## Check Claude routed-session discovery and preflight

Use a fresh private Router root, isolated `HOME` and `CODEX_HOME`, and unused loopback ports. This keeps the
check away from the installed Host and the owner's Claude session registry. Build the debug binaries from
the repository root:

```sh
cargo build -p codex-router-cli --bin codex-router \
  -p agent-collaboration --bin agent-sessions
```

Create a new root and its isolated homes and socket directory:

```sh
claude_proof_root="$(mktemp -d /tmp/claude-routing-proof.XXXXXX)"
mkdir -p "$claude_proof_root/home" "$claude_proof_root/codex-home" \
  "$claude_proof_root/native-socket"
chmod 700 "$claude_proof_root" "$claude_proof_root/home" \
  "$claude_proof_root/codex-home" "$claude_proof_root/native-socket"
```

In a foreground terminal, start the Host on two unused loopback ports. The Router proxy port below is an
example; use the same port consistently for this run:

```sh
env HOME="$claude_proof_root/home" CODEX_HOME="$claude_proof_root/codex-home" \
  CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET="$claude_proof_root/native-socket/app-server.sock" \
  ./target/debug/codex-router host --router-root "$claude_proof_root" \
  --port 43131 --mcp-bind 127.0.0.1:43132 --require-debug-isolation
```

The Host owns this unused Router port, provisions the local token before starting `serve`, and publishes its
configured proxy endpoint in `agent-communication/service.json`. Confirm the endpoint before running the
session commands:

```sh
rg -o '"routerProxyEndpoint":"[^"]+"' \
  "$claude_proof_root/agent-communication/service.json"
```

In another terminal, list Claude sessions and inspect the launch arguments. The debug root environment makes
`agent-sessions` use the same service directory as the Host:

```sh
env HOME="$claude_proof_root/home" CODEX_HOME="$claude_proof_root/codex-home" \
  CODEX_ROUTER_DEBUG_ROUTER_ROOT="$claude_proof_root" \
  ./target/debug/agent-sessions --provider claude --list --format json

env HOME="$claude_proof_root/home" CODEX_HOME="$claude_proof_root/codex-home" \
  CODEX_ROUTER_DEBUG_ROUTER_ROOT="$claude_proof_root" \
  ./target/debug/agent-sessions --provider claude --new --dry-run
```

The list combines active Router sessions with stored transcript metadata under the isolated `HOME`; it does
not read transcript content. Dry-run prints the `claude` command and does not contact Router or start Claude.
To check the stopped-Router boundary, stop this foreground Host with Ctrl-C, then run a non-dry routed launch
with the same environment and confirm it exits nonzero with the endpoint-not-published or Router-not-ready
error before Claude starts. The real-client request path through a fake Anthropic upstream remains the PR7
acceptance proof; listing and dry-run do not establish that request path.

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

Source: [debug Host launcher](../../crates/codex-router-host/examples/automation-debug-host.rs), [Claude launch target](../../crates/agent-collaboration/src/session_commands/claude_launch_target.rs), [Claude launch target tests](../../crates/agent-collaboration/src/session_commands/claude_launch_target_tests.rs), [Host Claude launch environment](../../crates/codex-router-host/src/claude_provider_launch_environment.rs), [Host token startup](../../crates/codex-router-host/src/lifecycle_owner/startup_convergence.rs), [live acceptance test](../../crates/agent-collaboration/tests/debug_luna_acceptance.rs), [scheduled workflow requirements](../specs/2026-09-07-scheduled-agent-workflows/2026-09-07-scheduled-agent-workflows-requirements.md).
