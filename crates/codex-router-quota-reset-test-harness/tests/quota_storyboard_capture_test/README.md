# Automated real quota storyboard experiment

Run from the repository root on the nominated macOS capture surface:

```sh
sh crates/codex-router-quota-reset-test-harness/tests/quota_storyboard_capture_test/run_capture.sh
```

This runs the actual compiled quota entrypoint, not a second implementation of
its screen. A capability-marked temporary fixture supplies synthetic accounts,
SQLite and the existing sealed loopback provider. The tool adapter rejects every
ttyd invocation except the verified version probe and exact VHS loopback flags;
it replaces the shell with one fixed, guarded quota executable.

The command runs wide 120x36 and narrow 76x30 journeys through Browse, Resets,
Credits refresh, explicitly selected Disallow and Allow drafts, cancelled policy
editing and return to Browse. It saves seven
actual PNGs per viewport and verifies each image's independent scene markers with
the local macOS Vision framework. A short frame-scheduling interval is not an
oracle: blank or wrong images fail, even when terminal-text waits pass.

After `q`, the tape waits for the adapter’s literal zero-exit marker, emitted
only after the actual child exits and its exit record is saved. Capture teardown
cannot substitute for that successful exit.

Each capture run independently checks ten GETs/no POST, persisted synthetic 42.00
balance, Disallow policies after cancelling Allow, unchanged secret manifests,
credential canaries, actual child dimensions/exit, and owned process/fixture
cleanup. Permanent negative checks reject the preserved actual blank startup PNG
and a Credits image asserted as Browse; a Disallow draft asserted as selected Allow
must also fail despite an unselected Allow option being visible. A separate
impossible-screen journey must
fail with the exact bounded screen timeout, not merely any launch error.
An interruption test drops the capture while its real child is active and proves
the owned child/ttyd stop, no provider requests occur, and its fixture is removed.

Outputs go to a new `tmp/quota-live-storyboard-<UTC>-<pid>/` directory. Set
`QUOTA_STORYBOARD_OUTPUT` to a fresh owned path to retain a specific run. Existing
scene directories fail rather than silently borrowing old images. Receipts bind
the compiled child, captured PNGs, driver/OCR source and tool binaries, viewport
and configured font file. Image-observation JSONs retain recognized text and
missing/unexpected markers for diagnosis.
The candidate Menlo file digest does not assert which face or fallback Chrome
actually rasterized; exact font and styled-cell fidelity remain unverified.

## Tool prerequisites and limits

This feasibility command expects the verified checkout-local bottles under
`tmp/quota-real-tui-tools-2026-10-12`: VHS0.12.1, ttyd1.7.7 Homebrew revision14 and
libwebsockets5.0.0 revision2. Existing Chrome, ffmpeg, json-c, libuv, OpenSSL4 and
Xcode/Swift tools are used. `QUOTA_STORYBOARD_TOOL_ROOT` may nominate that isolated
tool directory. No global install, source dependency, normal account access,
production Router restart or native model/Host launch is performed.

The tests are explicitly opt-in because the nominated capture tools and macOS
Vision runtime are outside the ordinary Rust test environment. This has not been
installed as a hosted-CI gate. The same host's actual permission controls must
permit the private Chrome/loopback run and Vision image-file reads. Prior denied
mock-page/native capture routes do not supply permission.

These are browser-rendered screenshots of live terminal output, not native
Ghostty screenshots, production quota data, exact styled-cell verification or a
complete general TUI automation framework. The existing Rust PTY resize/mouse and
credit-save tests remain required. Show/hide disabled accounts is a separate
product change; this experiment does not pretend the toggle exists.

The two imported read-only fixture modules expect the specific dead-code warning
for their additional scenarios unused here. No blanket warning allowance applies
to the driver, and the original full fixture consumer remains linted normally.
