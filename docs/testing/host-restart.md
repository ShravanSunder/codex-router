# Host restart validation

The three commands have separate jobs:

| Command | Effect |
| --- | --- |
| `codex-router host restart` | Replace the entire Host using the installed CLI issuing this command, retaining the running Host's root, port and launch configuration; wait for readiness. |
| `codex-router host app-server restart` | Restart managed Codex without updating it or replacing the Host. |
| `codex-router host app-server update` | Run the managed Codex updater; preserve the running runtime on no change or update failure; activate changed Codex through the existing Host replacement path. |

Installing a binary and replacing a running Host are separate actions. Restart
does not fetch a release. Native connections may disconnect during replacement;
local readiness and Remote Control degradation are reported separately.

## First activation and failure recovery

A Host that predates the whole-Host restart request needs one manual activation:
stop it from its owning foreground terminal, wait for that process to exit, then
launch the installed `codex-router host`. Do not race a second launch against
shutdown: the singleton lock intentionally rejects it. The CLI does not find or
kill a process to bypass this boundary, and `host update` is retired.

If replacement fails, read the terminal result and inspect the owning terminal.
Start `codex-router host` after the old Host has exited. A progress message or
closed connection is not successful activation. Do not replay a mutation merely
because its response was lost.

## Isolated proof

Use the permanent compiled CLI Host acceptance harness and Host lifecycle tests.
They exercise the real CLI, private operator socket, Host lifecycle, file lock,
process replacement and router. Fake managed Codex and launchctl fixtures isolate
the external process boundary; they do not replace the Host behavior being proved.

Keep all test roots and endpoints private and disposable. Never point an
acceptance run at the production router root or send signals to a Host outside
the fixture's ownership. Development installs use Cargo. Homebrew distribution
is verified by the release workflow, separately from process activation.

The cross-version proof uses two compatible candidate builds at different
package versions. It must verify the selected newer image, child settlement,
exclusive singleton ownership, readiness, and native attachment after restart.
Run both an atomic replacement at the same installed path and invocation from a
different installed path, as with versioned package-manager directories. A
same-binary re-exec or a passing parser test does not establish this result.
Legacy bootstrap is documented above and is not claimed as transparent protocol
compatibility.
