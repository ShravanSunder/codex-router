# Router default port migration packet

Prepared for review; activation has not run. Delivery target: reviewed current-head CI-passing PRs, unmerged. Source version reserved `0.1.88`; release/installation remain separate gates. Canonical plan and private operational details are retained in the owning worktree; this packet carries the public migration contract.

## Selected endpoint and boundaries

Production proxy endpoint: `http://127.0.0.1:19741/v1`. IANA currently marks enclosing19542–19787 unassigned; observed owner-machine macOS ephemeral range49152–65535. Snapshot inventory found no19741 listener. This reduces collision risk; it does not establish permanent availability or fix OAuth/token/quota/compaction behavior. Simultaneous OAuth collision has not been observed. Router OAuth source uses hosted Claude and OpenAI device-flow callbacks; this patch changes neither callback contract. Native/other-application callback listeners remain unobserved.

Isolated Host default18787, maintained explicit debug43127, collaboration MCP8788, native/Host Unix sockets, upstream provider URLs and OAuth callback ports keep their owners. Explicit `--port` overrides remain supported.

## Activation ownership preflight

Capture current listener PIDs, process ancestry, executable identity and selected nonsecret launch flags immediately before activation. A reviewed private ownership snapshot must identify the foreground Host and its Router/app-server children; PIDs are observations, never authority to signal. Preserve unrelated debug instances and checkpoint affected sessions.

The deployment command below reflects the established root, MCP bind and retention policy. Preserve any additional owner/provider launch flags established by preflight. Cross-machine listener inventory and native acceptance remain separate activation gates; port availability can change after a snapshot.

## Why activation needs a cold start

CLI Host operator dispatch sends `RestartHost { executable }` only. The Host replacement command retains effective `--port`; lifecycle admission changes executable only. `host restart --port 19741` therefore preserves the current8787 listener. `host router restart` uses the existing child launch plan. Source anchors: CLI host_command/mod.rs and foreground_launch.rs; Host lifecycle_owner/request_admission.rs; docs/testing/host-restart.md.

Activation after explicit approval must stop the verified owning foreground Host, wait for complete child shutdown and listener release, then start the installed Host once with19741. The native app-server and Router collaboration sessions reconnect; direct clients that cached8787 need to reopen with the migrated production profile. MCP remains8788 but its Host stops during this controlled interval. Save work/checkpoint dependent agents before the window.

The acceptance harnesses' production identity guard follows the canonical default19741. Before activation, after rollback to8787, or with another explicit production port, these harnesses fail closed instead of proving preservation of that custom listener. Use the current private ownership preflight for the actual running Host; no harness success is inferred during this transition.

## Release and staged prerequisites

1. Merge only after explicit merge authority, current review/check/thread/head/mergeability gates. Source package0.1.88 and Cargo.lock are required before merge.
2. Publish matching release/tag under separately authorized release scope, run required release artifact/tap/install checks. Homebrew installation does not authorize process replacement.
3. On each authorized target, install via `brew update` then `brew upgrade codex-router`. Verify installed binary reports0.1.88 and the release/tap evidence matches. Leave running Host intact.
4. Review production profile endpoint-only delta. The maintained profile repository isolated PR changes only base_url8787→19741 and private changelog. Preserve any concurrent model/provider/configuration work in the maintained checkout; coordinate its owner before updating it. Use targeted application after reviewing the live delta.
5. Preserve existing live/source model/provider settings; target only the approved production profile endpoint. Targeted chezmoi diff/apply is allowed only once selected source includes the preserved WIP and live preview confirms the intended change. Never apply the baseline generated profile over user settings. Debug source/live stay explicit43127.

## Concrete activation command set (requires owner approval)

In the verified owning foreground terminal, after all prerequisites and saved agent checkpoints:

```sh
/usr/sbin/lsof -nP -iTCP:8787 -sTCP:LISTEN -Fpcn
/usr/sbin/lsof -nP -iTCP:19741 -sTCP:LISTEN -Fpcn
/opt/homebrew/bin/codex-router --version
```

Stop the verified foreground Host using its terminal's ordinary interrupt; wait for that Host and owned Router/app-server children to settle. Reverify8787/MCP8788 are released and19741 is free; never kill an unknown borrower. Then start once:

```sh
/opt/homebrew/bin/codex-router host \
  --router-root "$HOME/.codex-router" \
  --port 19741 \
  --mcp-bind 127.0.0.1:8788 \
  --provider-operation-retention-days 60
```

Preserve any additional verified production provider/owner flags rather than inventing them. Reopen direct clients only after profile and listener agree.

## Acceptance observations

- `codex-router host status` readiness and selected native routing agree with19741.
- `curl --fail --silent --show-error http://127.0.0.1:19741/healthz` returns the real static health contract.
- Listener ownership/parentage identifies newly started Host and its child; no old8787 listener remains owned by this Host.
- Parsed production profile endpoint19741, other config fields preserved, private mode0600.
- MCP8788 discovery still identifies the same service/endpoint identities and existing conversation/session storage. Resume one explicitly chosen existing session through supported native selector and observe real model request acceptance; no auth refresh/token export.
- Observe an existing unaffected OAuth callback listener only if separately authorized and available; source separation alone does not establish a native OAuth journey.

## Rollback (same approved deployment window)

If startup/profile/client acceptance fails, collect bounded sanitized failure evidence. Stop only the newly owned foreground Host and wait for child settlement. Restore the profile endpoint-only change19741→8787 while preserving all other fields. Confirm8787 free, then start the installed binary once with `host --router-root "$HOME/.codex-router" --port 8787 --mcp-bind 127.0.0.1:8788 --provider-operation-retention-days 60`, preserving verified additional flags. Explicit overrides work with the new binary, so no binary downgrade or state/credential migration is needed. Reopen affected clients and verify real health/native acceptance at8787. If another app owns8787, stop and bring that evidence to the owner.

## Isolated cutover rehearsal

The permanent default-bind acceptance test is opt-in locally because the production default may be occupied. A mandatory explicit CI step runs it with `--ignored` on a controlled runner; it fails if19741 is occupied and never touches a borrower. The required rehearsal uses one owned real CLI/config instance with fresh private state: old explicit unused proof port/profile, owned stop and listener release, profile cutover to19741, real default serve at19741, `/healthz`/appropriate no-model request, and owned teardown proving both listeners free. It uses the existing CLI boundary and establishes the port/config interaction. Native Host/app-server and OAuth journeys require separate acceptance and are not inferred. Permanent test: `crates/codex-router-cli/tests/default_router_port_cutover.rs` (requires `keychain-test-support`). Observed command: `RUSTC=<pinned-toolchain-rustc> rustup run 1.98.1 cargo test --locked --offline -p codex-router-cli --features keychain-test-support --test default_router_port_cutover compiled_cli_default_router_port_cutover_uses_isolated_state -- --nocapture`, exit0,1passed/0failed. Latest owned phases: old private proof port53211 → default19741; each returned health200, unauthenticated401 and valid-local-token empty-account503. Both children exited successfully; old/new ports were rebindable. Parsed new generated profile SHA-256: `e1c3aa5b26f39adf865cf477394a75ac6505bd73273974d7a6bd5bc0c161079a`. Actual typed commandExecution output recovered through supported native thread/read into ignored local evidence `tmp/default-router-port-command-proof.json`; no provider transcript files read and no duplicate retention rerun. Evidence was produced on the source captured in implementation commit103068f8; final corrective source/commit linkage and CI receipts will be attached before readiness.

## Evidence and outstanding gates

The maintained profile repository targeted chezmoi isolated diff/apply/readback passed: endpoint19741, all other baseline fields preserved, mode0600. Scoped Router red/green and actual isolated CLI/config runtime outputs are inspected. Formatter, file-size and scoped Clippy outputs were inspected; fresh affected checks verified the shared guard consumers. Independent review, PR and current-head CI gates remain pending. Installed/live endpoints remain unchanged. Other-machine inventory and actual OAuth collision remain unverified. No credentials/sessions/production processes changed.

Final deployment approval must come from the owner using the repository's required instruction: **replace the production Codex router process**. Attach exact PR/proof identities before PR readiness. Release, installation and fresh production ownership checks remain prerequisites before any approved activation; they have not run.
