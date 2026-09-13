# Host replacement through the installed CLI

The [Requirements](requirements.md) and [Specification](specification.md) separate
Host replacement from managed Codex lifecycle. The existing lifecycle owner
remains the sole process owner; the CLI selects the installed executable and
observes completion.

## Ownership and call changes

The source paths in the current/target comparison describe baseline commit
`0c5c1b8`; the target column identifies the replacement behavior.

```text
CLI host command                         Host lifecycle owner
 owns parsing, current_exe, output        owns config, children, mutation state
   |
   | NEW RestartHost { executable } over private operator socket
   v
request admission -> shared Host replacement activation
                      | stop collaboration publication
                      | settle app-server, then owned router
                      | flush telemetry, unpublish operator socket
                      | transfer continuously held lock, exec
                      v
                    replacement Host
                      | validate inherited lock and protocol marker
                      | restart children using retained root/port/environment
                      | publish collaboration and operator readiness
CLI <-- EOF + progress; AwaitHostStart --> replacement Host
    <-- ready / local-ready-remote-degraded / failure
```

| Source-backed current path | Target delta and consequence |
| --- | --- |
| `host_command/mod.rs`: `host restart` -> `RestartAppServer` -> `request_admission` -> `explicit_app_server_restart` | Change the CLI spelling to `host app-server restart`; preserve child-only lifecycle and terminal response. |
| `host update` -> `UpdateCodex` -> `codex_update_preparation` -> `update_activation` | Change spelling to `host app-server update`; preserve no-change/failure behavior and changed-update activation. |
| Changed-update activation -> retained child teardown -> `HostInstance` lock transfer -> `exec` | Share this existing activation with explicit Host restart. Generalize its names and carry the initiating operation for outcomes and telemetry. No second replacement implementation. |
| `foreground_launch.rs` captures `current_exe()` for its replacement command | Explicit restart replaces only that command's executable with the invoking CLI's absolute `current_exe()` path. Retain Host-owned arguments, environment and output. This handles different versioned install directories. |
| `host_singleton_authority.rs` compares a package-version marker | Use a fixed versioned handoff-protocol marker, independent of Cargo package version; preserve descriptor/device/inode validation and uninterrupted lock ownership. |
| `operator_client.rs` accepts replacement progress followed by EOF; `update_outcome.rs` reconnects with `AwaitHostStart` | Reuse the same bounded observation for restart; render restart success/failure without claiming Codex was updated. EOF alone is never success. |

The private request adds `RestartHost { executable: PathBuf }`. Requests become
cloneable rather than copyable. The field is supplied by the CLI, not a public
replacement-command flag. The Host accepts an absolute regular executable file,
and rejects invalid paths or missing replacement configuration before taking
children. The request cannot replace argv, root, port, environment or launch
policy. The owner-private Unix socket remains the trust boundary; this is not
authentication against other processes of the same OS user.

The Host operation enum adds `RestartHost`. Shared activation retains its
originating request and operation so failure, busy, shutdown and telemetry use
the correct meaning. A shared replacement-readiness observer in the CLI owns
reconnect and readiness interpretation. Restart has its own result vocabulary;
the existing four update results remain available for app-server update.

## State, failure and concurrency

All state below belongs to the existing single Tokio lifecycle owner and lasts
only for the foreground process. No database or persistent operation record is
added.

| State and event | Transition and observable result |
| --- | --- |
| Steady, valid restart request | Enter `Mutating(RestartHost)` and retain the shared activation future; emit replacement-starting progress. |
| Another mutation, recovery, pending updater/identity drain or status observation owns required handles | Return busy; do not take children or dispatch another restart. |
| Missing command or invalid executable | Terminal failure before teardown; current children remain retained. |
| Activation begins | Close collaboration through its existing owner before polling teardown; stop app-server before owned router. External router remains outside lifecycle ownership. |
| Child cannot be stopped | Retain surviving handles, return failure, never exec over a live owned child. Existing lifecycle can report/recover the remaining state. |
| Child settlement succeeds | Flush telemetry, remove operator pathname, prepare inherited descriptor, exec the selected executable. |
| Exec fails | Release the prepared stdin duplicate and return the existing foreground fatal error; CLI reports replacement failure and manual recovery. |
| Replacement bootstrap/readiness fails | CLI's bounded observation reports failure with recovery guidance. Do not replay the mutation or discover/signal a PID. |
| Signal branch takes ownership before activation completion is selected | Existing shutdown settlement awaits the retained teardown future and terminates without exec. Once the owner selects completed activation and begins finalization, the existing bounded telemetry/exec path is committed and no longer polls the signal branches. |
| Concurrent new Host | Stable file lock denies acquisition before socket removal, including across exec. |

The initial restart exchange covers existing complete child teardown bounds;
the existing 40-second replacement-readiness deadline begins after old-Host EOF.
Only connection attempts for the read-only readiness observation retry, and only
while the replacement endpoint is absent/refused. A request transport error is
reported, not retried. Remote Control degradation remains distinct from local
unavailability. Progress and failures must not use "updated Codex" for restart.

The existing collaboration shutdown releases listener publication and closes its
runtime before exec; the replacement reopens the same persisted state and
rebuilds publication. No schema or credential changes occur. Existing bounded
operator frames, connection limits, queueing and telemetry redaction remain.

## Cutover and tradeoff

The old CLI spellings are removed directly. A pre-feature Host cannot decode
the new request. Documentation tells the owner to stop that foreground Host in
its owning terminal, wait for exit, then start the newly installed binary once.
An incomplete restart response may suggest this diagnosis without asserting
that every transport failure is an old Host. There is no compatibility shim.

Shared same-process exec is chosen because the Host already owns ordered
teardown, configuration and continuous lock transfer. A separate supervisor or
CLI-driven stop/start would add another owner and a lock-release gap. The cost
is intentional connection interruption and no automatic rollback after exec
failure; the owner performs the documented recovery. Revisit only if the product
requires uninterrupted sessions or an independently managed service supervisor.

## Proof boundaries

```text
isolated driver -> compiled CLI -> real private Unix protocol
                                  -> real Host lifecycle / lock / exec
                                  -> real owned router child
                                  -> fake managed Codex and launchctl
driver observes: binary identity, PID, child generations, updater invocation,
                 contention failure, readiness and post-restart attachment
```

| Specification proof | Realization and observable seam |
| --- | --- |
| V1 / U1,U3,U4 | Two compatible candidate builds at distinct package versions, old Host launched from one path and restart invoked from another installed path. Observe selected replacement image, continuous singleton exclusion, stopped old children, established endpoints and final CLI readiness. Also replace an installed path atomically to exercise normal Cargo installation. |
| V2 / U2,U6 | CLI parser/help contracts for all three commands and rejection of retired `host update`; existing router-restart contracts. |
| V3 / U2,U3,U4 | Existing updater matrix and child restart fixtures retain no-change/failure/changed activation coverage; shared replacement tests cover invalid executable, busy admission, teardown failure, shutdown taking ownership while teardown is retained, and failed readiness. |
| V4 / U5,U6 | Stable protocol-marker test independent of package version plus malformed marker/wrong-descriptor rejection; documented legacy bootstrap and source inspection for absence of PID fallback. |

The fake Codex supplies native readiness and controlled updater outcomes; it does
not replace the Host, CLI, Unix socket, file lock, process exec or router under
test. Candidate build versions prove supported-version handoff, not transparent
upgrade from the incompatible legacy Host. Production state and processes are
outside all proof fixtures.
