# Homebrew Autopublish

## Outcome

A trusted `v<major>.<minor>.<patch>` tag on `codex-router` publishes a prebuilt Apple Silicon macOS binary and updates `ShravanSunder/homebrew-taps` without a local release step.

The workflow must also support an explicit manual run for an existing tag so the release path can be retried without creating another version.

## Release boundary

`codex-router` owns building and publishing its release artifact. The workflow:

1. checks out the tagged commit;
2. requires the tag version to equal the workspace Cargo version;
3. runs on the pinned `macos-15` arm64 image and fails unless `uname -m` reports `arm64`;
4. builds only the `codex-router` release binary with the locked Rust 1.95.0 toolchain;
5. verifies the binary is Mach-O arm64 and reports the expected version;
6. packages `codex-router-v<version>-aarch64-apple-darwin.tar.gz` and computes its SHA-256 digest; and
7. creates or updates the matching GitHub release using the source repository's scoped `GITHUB_TOKEN`.

The release must remain unpublished when any version, architecture, build, or smoke check fails. Retrying the same tag replaces only the expected versioned asset and does not create a second release.

## Tap boundary

`homebrew-taps` owns the formula. After the release asset exists, the source workflow checks out the tap through a dedicated write-enabled deploy key restricted to `ShravanSunder/homebrew-taps`.

The updater changes only `Formula/codex-router.rb`:

- release URL;
- SHA-256 digest; and
- source commit provenance.

It preserves the formula's explicit Apple Silicon macOS requirements and direct binary installation. The workflow runs Ruby syntax validation, Homebrew style, strict audit, installation, formula test, linkage validation, architecture validation, and version validation before committing the formula update.

If the formula already contains the requested release URL, digest, and provenance, the tap phase succeeds without creating an empty commit. Otherwise it creates and pushes one release-specific commit directly to the tap's `main` branch.

## Credentials and permissions

- The release job receives `contents: write` only for `codex-router` through `GITHUB_TOKEN`.
- The tap deploy key grants Git write access only to `ShravanSunder/homebrew-taps`.
- The private key is stored as the `HOMEBREW_TAP_DEPLOY_KEY` Actions secret in `codex-router`.
- The workflow is triggered only by trusted tags or an explicit manual dispatch, never by pull-request code.
- No credential content appears in workflow output, release notes, or repository files.

## Concurrency and failure behavior

One publish run may execute per release tag. A later retry for the same tag cancels no different-version release and cannot update the formula until its release artifact is available and verified.

Release publication and tap publication are deliberately ordered but not transactional. If the tap phase fails, the GitHub release remains valid and rerunning the same tag repairs the tap without rebuilding a different artifact contract.

## Proof gates

Before the automation is considered complete:

1. workflow syntax and repository action lint pass;
2. updater tests prove exact-field replacement, no-op idempotency, and invalid-input rejection;
3. a manual `v0.1.2` run completes on GitHub's arm64 runner;
4. the release contains exactly the expected arm64 asset and digest;
5. the tap workflow phase authenticates through the deploy key and pushes the provenance-bearing formula update;
6. Homebrew style, strict audit, install, formula test, linkage, architecture, and version checks pass from the updated tap; and
7. the running production router process is not stopped, restarted, or replaced.
