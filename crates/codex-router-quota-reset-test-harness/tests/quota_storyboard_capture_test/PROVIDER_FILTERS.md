# Real provider-filter storyboards

This drives the compiled session picker and quota components through a real PTY.
Input inventory is synthetic: six typed session records and four real fixture
SQLite account rows. It does not launch a session or contact a live vendor.

Sessions choose All, Codex, Claude Code or Cursor. Quota chooses All, OpenAI or
Claude and independently shows/hides disabled accounts. Ctrl+F/F3 opens the chooser;
arrows change the provider, Tab changes focus, Enter applies, Esc cancels. Quota's
Space key toggles disabled visibility when that control has focus. Mouse controls
use the same state transitions. An existing modal owns input and its original
space; filtering cannot enable accounts or change launch identities.

From the repository root, nominate approved private resources:

```sh
export QUOTA_STORYBOARD_TOOL_ROOT=<verified-private-tools>
export QUOTA_STORYBOARD_OUTPUT=<fresh-owned-output>
export CARGO_TARGET_DIR=<private-capture-target>
export TMPDIR=<short-owned-temp-path>
sh crates/codex-router-quota-reset-test-harness/tests/quota_storyboard_capture_test/run_provider_filter_capture.sh
```

The runner uses the pinned, already supplied VHS/ttyd libraries and builds the
Swift Vision image checker. It installs nothing. Keep TMPDIR short because the
existing Unix-socket test fixtures have the platform socket-path limit.

The current runner selects six compact journeys: sessions and quota at 120x36,
76x30 and 48x24. Each captures browse, the provider dialog, contextual Help,
the applied provider, a cancelled draft and details (36 images). Quota's applied
scene also hides disabled accounts, preserving the provider/visibility intersection.
Help sits on the title row; shortcut text appears only inside Help. The Help
journey attempts background input, closes back to the unchanged draft and then
applies it. Existing key bindings still drive the real UI.

The compact tape contract is a permanent test with literal checkpoint identities.
Image oracles require literal active values/results and reject blank, wrong-provider
and chooser-as-applied PNGs. Literal iocraft Canvas tests separately prove heading,
selection and focus styles. Text recognition alone cannot establish those styles.
The earlier full capture entrypoints and their archived95 images are historical
coverage; this runner does not relabel those pictures as current-source proof.

Session details retain the existing140-column sidecar threshold, and quota retains
160columns; narrower interactive screens open details explicitly in a dialog.
Dialogs keep title/actions visible while their body scrolls. Provider session
details show their actual ID/endpoint/state rather than inventing native conversation
history or availability.

Every run creates fresh directories. Receipts bind the compiled binary, driver,
image-checker source/binary, tools, tape, viewport and PNG hashes. Filtering must
leave SQLite/secret bytes unchanged, make zero synthetic provider requests, exit
successfully and reap its child/wrapper/ttyd and private headless-browser processes.
VHS uses a separate owned temporary profile directory per capture; cleanup binds
browser PIDs to that private prefix and UID, never an ordinary browser. A failed startup
is a failed test, with cleanup and an exact-owned-process diagnostic; there is no
retry loop or timeout increase. Text recognition is not a full styled-cell or
native Ghostty rendering claim.

Default CI also runs the real stored-provider/disabled-intersection process test
in `quota_reset_pty_test/provider_filter_pty_test.rs`. It needs no browser tools.
Unit tests cover literal identities, classification, focus, reload, cancel, mouse
parity, modal priority and responsive/static composition. Those checks and these
screenshots have separate roles: a PNG cannot establish an off-screen identity
partition or live provider/native acceptance.

On a failed classic quota capture, safe HTTP method/path/status and stored refresh
status presence/error classes are retained before the fixture is removed. These
records omit account identities, credentials and raw errors. A passing diagnostic
rerun does not explain an earlier failure whose fixture evidence was lost.
