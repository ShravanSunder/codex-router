# Debug profile validation and proposed acceptance correction

> Historical investigation, resolved by the validated debug-profile projection in [debug_profile_projection.rs](../../crates/codex-native-integration/src/debug_profile_projection.rs) and [app_server_launch.rs](../../crates/codex-native-integration/src/app_server_launch.rs). The custom upstream debug-build proposal below was not adopted. Current supported testing instructions are in the [agent communication guide](./2026-09-06-agent-communication-guide.md).

The current debug app-server launch cannot run with installed Codex 0.153.4. The canonical implementation plan remains unchanged; its debug launch/proof mechanism needs correction before further implementation depends on it.

## Assumption and observed result

The plan requires real launches to use `codex-router-debug`, normal Codex home, and a dedicated debug socket. `AppServerCommandSpec::with_profile` prepends `--profile codex-router-debug` before `app-server`.

The installed executable reports `codex-cli 0.153.4`. This invocation exited 1 before app-server startup:

```text
codex --profile codex-router-debug app-server --listen unix:///private/tmp/codex-profile-rejection-proof/backend.sock

Error: --profile only applies to runtime commands and `codex mcp`: ...
```

Official release commit `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a` establishes why: `codex-rs/cli/src/main.rs:1087` calls `profile_v2_for_subcommand` before dispatch; line 1824 excludes AppServer from profile-supported commands. The direct app-server branch passes `LoaderOverrides::default()` to the runtime.

Source: https://github.com/openai/codex/blob/3d2ee51ca2d5db578f328aa75e20aa22c0197c9a/codex-rs/cli/src/main.rs

The named debug configuration file exists. It is a profile-v2 file, not a legacy `[profiles]` entry; absence from the main file's legacy profile table is not a configuration defect. No configuration content or secrets were copied here.

Argument-construction tests prove what our code emits, not that Codex accepts it. The live rejection supersedes any inference that debug launch is operational.

## Startup lock clarification

The release's `app-server/src/lib.rs:600` acquires the Codex-home startup lock before preparing the Unix listener. Line 756 drops it after binding the listener. This is startup serialization, not evidence that different sockets can never coexist. A research tool's stronger lifetime-singleton claim was rejected against source. Concurrent-runtime safety still needs actual debug proof and inspection of shared-state startup.

Source: https://github.com/openai/codex/blob/3d2ee51ca2d5db578f328aa75e20aa22c0197c9a/codex-rs/app-server/src/lib.rs

## Proposed proof route, not adopted

Build the exact, unmodified release source in debug mode in an isolated build checkout. The existing upstream `CODEX_APP_SERVER_TEST_USER_CONFIG_FILE` hook selects the existing debug profile file through `LoaderOverrides.user_config_path`. Keep normal Codex home and a dedicated owner-controlled socket. Omit the unsupported app-server `--profile` argument. Keep named-profile selection for native TUI launches, where upstream supports it.

The hook is defined at app-server/src/lib.rs:151, read only under `cfg(debug_assertions)` at lines 1338–1347, and applied at lines 1350–1372. Upstream's `debug_test_user_config_file_overrides_loader_path` test covers this override. Installed release builds ignore the hook; setting it on the current release executable is not a fallback.

Costs and limits: a native debug build may be expensive, changes the acceptance executable, and does not prove the installed release supports this route. Executable/schema identity and the CLI debug override must be explicit in a new plan. No upstream changes, global configuration writes, production replacement or fake Codex home are proposed. The current local upstream checkout has a different HEAD and cannot be treated as the exact release build source.

The owner decision is whether to adopt this route or supply another supported route. Until then, debug launch is not claimed usable and public ACP publication remains disabled.

## Independent validation

Setup-routing and prompt-publication regressions passed. Native server envelopes compile against the saved 0.153.4 schema export and reject malformed events. Cancellation reload tests preserve the barrier while the target is active and clear it after an idle snapshot. These are source/schema/socket-fixture results, not real debug Host/TUI/ACP acceptance.

Read-only process inventory was denied by the execution sandbox; no production PID/status inference was made. No production process was stopped, restarted or replaced.
