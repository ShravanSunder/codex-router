---
name: verify-codex-router
description: Verify a declared signed debug Codex Router through its real Host, native stored-push correlation, typed provider Question and Approval callbacks, and owning SQLite reopen. Use for scoped local verification, not delivery acceptance.
---

# Verify Codex Router

Drive an isolated debug Router and its real owning Host, preserving normal Codex home. All helper code lives once in `scripts/verification/`. Run from the repository root.

For model-free private Tailnet MCP/board verification, follow the separate [Tailnet journey](references/tailnet-mcp.md) and its HostPlan/BoardPlan launch. That journey is draft until exercised on a declared real target.

**Observed locally:** genuine Claude Question/Approval callbacks, actual Session test decisions, marker execution, owning SQLite reopen and exact cleanup on Sunclaw. The first unreachable native-Approver case remains negative evidence. **Portable cold entry and Cursor remain draft:** endpoint/settings/tool parameterization has structural and retained-Claude qualification; no Cursor model input or callback run has happened. Use [Cursor qualification](references/cursor-provider.md) before its first input. No successful journey is rerun merely to refresh the recipe.

## Launch and readiness

Get the caller's declared source, normal signed artifacts, allowed debug root/profile, provider executables and private evidence directory. This recipe adds no permissions. Use the actual host permission mechanism if authorized process/state access is sandbox-blocked; do not switch identities, roots or transports around denial. Keep production and other controllers untouched.

Python 3.13 is the supported apparatus interpreter (the pure ownership module uses Python 3.12 type-alias syntax). Read `RuntimePlan` in `scripts/verification/owned_runtime.py`. Put its JSON in the run's private evidence directory and set `RUNTIME_PLAN` to that path. Required fields: `repository`, `source_head`, `qualification`, `router_root`, `evidence`, `port`, `original_port`, `expected_version`, `profile`, `claude_node`, `claude_adapter`, `claude_executable`, `cursor_executable`, timezone-aware `deadline`. Resolve installed executables on this machine. `claude_executable` is an expected binding: preflight requires its canonical path to equal the first qualified global `claude` on the unchanged Host PATH and records that canonical path and SHA256 in `driver-binding.json`. It does not override the Host's selection. `runtime_qualification.py` mirrors `global_claude_executable.rs`, including package search directories, transient package targets, SDK exclusions and effective execute access; a normal global Claude Code package target remains eligible. Mismatch fails before any Router requester. Qualification also supplies exact normal binary paths/hashes and empty `testGuardFeatures`; the caller declares any source/build equivalence. Never copy old run IDs or machine install paths into a portable plan.

```sh
uv run --offline --no-project --with pydantic --python 3.13 python -m scripts.verification.owned_runtime "$RUNTIME_PLAN"
```

Use a persistent execution context with the actual required host grants. The helper checks source, strict debug signature/hash, process access, root ownership, database consumers and the selected free port before launching. It retains one Router across Host A/B. Preserve an exact live OS requester if approval is pending; never spawn another. The hash-guarded debug-profile port edit is required by `DebugCodexProfile::parse`, and original bytes are restored at cleanup.

Wait for `router-ready.json` and `host-A-ready.json`, then run the independent read-only check:

```sh
uv run --offline --no-project --with pydantic --python 3.13 python -m scripts.verification.owned_runtime "$RUNTIME_PLAN" --check
```

It checks recorded PID/UID/birth/group/executable, binary hash, health version and same-root Host publication. Health is readiness, not credential usability; a genuine provider journey proves that separately. `/v1/models` is not a Claude credential oracle. Launch owners: `crates/codex-router-cli/src/host_command/{mod,foreground_launch}.rs`, `crates/codex-native-integration/src/debug_profile_projection.rs`, `crates/codex-router-host/src/lifecycle_owner/startup_convergence.rs` and `claude_provider_launch_environment.rs`.

## Journeys

| Journey | Driver and deciding evidence |
| --- | --- |
| Private Tailnet boards and exposure lifecycle (draft) | [Tailnet MCP](references/tailnet-mcp.md): reciprocal real peer HTTPS operations, destination-local Unix/SQLite readback, Host/Origin controls and exact owned cleanup; separately declared after-ready exposure-loss negative. |
| Stored native push | `native_correlation.py --help`: supply qualified CLI, explicit service directory, exact thread, accepted turn, Push ID and fresh output path. Only native initialize + `thread/read includeTurns`. Require one matching `userMessage.clientId` in the accepted turn; native `id` is separate. Retain IDs/count/text hash only. This does not run the complete native held/wake/full-body/order matrix. |
| Real Question + private marker Approval | Follow [typed callbacks](references/typed-callbacks.md). Real emission, actual offered-option IDs, actual Session test actor, exact harmless marker command, actual allow-once option, provider callback/tool/turn and terminal rows. No fabricated Human or persistent grant. |
| Owning SQLite reopen | After all exact operations/callers/observers settle, close Host A, start Host B on the same root/binary, and read identical public/SQLite terminal records, creation instants and global revision. Storage-only reopen or an acknowledgement is insufficient. |
| Screenshots | `harness_report.py --help` serves retained scoped evidence on loopback from `claims.json` (`rows`, relative `details`, `scope`) plus this run's actual Router/Host metadata. Capture only the owned report through CUA, save original screenshot bytes and label it **harness UI**. It is not a native app screenshot. Keep denied native UI as a gap; no alternate app/capture-tool bypass. |

Adopted outcomes: interaction SQLite `requirements.md`, `specification.md`, `program-design.md` under `docs/specs/2026-10-04-interaction-sqlite`, especially R1/R4/R5/R6/R7; push `docs/specs/2026-09-30-router-push-format/specification.md` R15/R20. The owner's assignment determines permitted test actors.

## Reopen and cleanup

After exact callback/tool/turn and operation settlement, close only the synthetic provider Sessions; verify their terminal/applied/confirmed close operations. Write `reopen.json` in runtime `evidence` to request Host A close and B startup. Host B starts only after that explicit request and within the allocation deadline. Deadline expiry during readiness, waiting or between phases, and any unexpected Host exit (including exit 0), produce a non-null lifecycle failure and cannot authorize B. `stop.json` requests cleanup without a reopen. Require A exit 0/no errors/no remaining owned processes, then B readiness with a new epoch and the same service/root/signed binary. The lifecycle verifies that A's database consumers, sockets and publication are gone before B starts; it keeps the external Router alive.

`interaction_readback.py --help` reads scoped rows and the singleton global revision in one read-only transaction. `--before` accepts only identical answered/decided records, creation instants, database root and revision. Query the same exact IDs publicly through Host B too. Exact cancelled-row equality can prove negative-history persistence, never successful callback consumption.

For exact recorded-inventory cleanup, `process_inventory.py --help` provides independently runnable `--capture-root <this-run-host-started.json> --inventory-output <fresh-file>` and `--settle-inventory <recorded-file>` actions with the runtime plan. It checks stable PID/UID/birth/group/executable, excludes mutable parent from identity, acquires descendants only from matching roots, and tolerates an independently disappeared PID. Capture again after all callers settle before planned cleanup. Keep the separately retained Router out of Host A's inventory. No inventory grant permits signalling an unrecorded process.

After B readback, write `stop.json` in runtime `evidence`. Inspect `lifecycle-settlement.json`: require no remaining owned processes/cleanup errors, exact profile restoration and surviving evidence. Stop only exact recorded report-server processes after fresh identity checks and close only the owned tab. Never kill by process name or remove evidence directories. Retain every failed/uncertain operation ID.

## Apparatus gates and return

```sh
uv run --offline --no-project --with pydantic==2.14.1 --python 3.13 python -m unittest scripts.tests.test_verification_observations scripts.tests.test_verification_lifecycle -v
uv run --offline --no-project --with pydantic==2.14.1 --with basedpyright==1.40.2 --python 3.13 basedpyright --pythonversion 3.12 --pythonplatform All scripts/verification scripts/tests/test_verification_observations.py scripts/tests/test_verification_lifecycle.py scripts/tests/verification_controller_fixture.py
uv run --offline --no-project --with ruff==0.17.0 ruff check scripts/verification scripts/tests/test_verification_observations.py scripts/tests/test_verification_lifecycle.py scripts/tests/verification_controller_fixture.py
uv run --offline --no-project --with ruff==0.17.0 ruff format --check scripts/verification scripts/tests/test_verification_observations.py scripts/tests/test_verification_lifecycle.py scripts/tests/verification_controller_fixture.py
```

These are apparatus checks, not product proof. Return machine/build/source/root/client, genuine IDs, actual callbacks/tool/turn, public/SQLite row/revision/creation-time readbacks, owning reopen, screenshot kind, commands/exits and cleanup. Mark every claim supported, unsupported or unknown, naming harness defects and unreachable routes separately. Acceptance remains with the Lead.

The observation views and unit suite require Pydantic v2. Typed views validate only used fields; complete raw snapshots remain authoritative for exact equality, including unknown fields and nanosecond timestamp strings. Controlled lifecycle tests use only recorded harmless local subprocesses and a synthetic pipe fixture; those results never count as native/provider runtime proof.
