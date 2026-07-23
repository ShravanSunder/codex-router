# Homebrew Autopublish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish a verified Apple Silicon macOS release and update `ShravanSunder/homebrew-taps` automatically whenever a trusted semantic-version tag is pushed.

**Architecture:** A tag/manual release workflow uses repository-scoped `GITHUB_TOKEN` authority for the source release and a separate Ed25519 deploy key whose only authority is pushing `homebrew-taps`. A tested Python updater owns exact formula mutations; the workflow refuses mismatched versions or architectures, verifies the formula through Homebrew before pushing, and is idempotent for retries.

**Tech Stack:** GitHub Actions, Rust 1.95.0, Python 3 standard library, Homebrew, GitHub REST API, Ed25519 SSH deploy key.

## Global Constraints

- Release artifacts support Apple Silicon macOS only.
- The accepted tag syntax is exactly `v<major>.<minor>.<patch>`.
- The tag version must equal the `codex-router-cli` Cargo package version.
- `GITHUB_TOKEN` may write only the `codex-router` release.
- `HOMEBREW_TAP_DEPLOY_KEY` may write only `ShravanSunder/homebrew-taps`.
- No secret value, private-key material, resolved credential, or credential path may enter logs, commits, release notes, or formula content.
- Installing or publishing must not stop, restart, or replace the production router process.
- Preserve the unrelated untracked `docs/specs/2026-07-12-shared-codex-app-server-host.md` file.

---

### Task 1: Formula updater with red-green proof

**Files:**
- Create: `scripts/tests/test_update_homebrew_formula.py`
- Create: `scripts/update_homebrew_formula.py`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: `update_codex_router_formula(*, formula_text: str, version: str, sha256: str, source_commit: str) -> str`
- Produces CLI: `python3 scripts/update_homebrew_formula.py --formula <path> --version <x.y.z> --sha256 <hex> --source-commit <hex>`
- Preserves formula invariants: `depends_on arch: :arm64`, `depends_on :macos`, and `bin.install "codex-router"`.

- [ ] **Step 1: Write failing updater tests**

Cover exact URL/SHA/provenance replacement, second-run idempotency, malformed SHA rejection, malformed version rejection, and refusal to mutate a formula without the Apple Silicon invariant.

- [ ] **Step 2: Run tests and verify RED**

Run: `python3 -m unittest scripts.tests.test_update_homebrew_formula -v`

Expected: import failure for missing `scripts.update_homebrew_formula`.

- [ ] **Step 3: Implement the minimal updater**

Validate inputs with full-match regular expressions, require the three formula invariants, replace exactly one URL and SHA field, replace or insert exactly one `# Source commit:` line, and atomically replace the formula only when content changes.

- [ ] **Step 4: Run tests and verify GREEN**

Run: `python3 -m unittest scripts.tests.test_update_homebrew_formula -v`

Expected: five tests pass with exit code 0.

- [ ] **Step 5: Add the focused suite to CI**

Add a `Homebrew formula updater tests` step before the workspace Rust tests in `.github/workflows/ci.yml`:

```yaml
      - name: Homebrew formula updater tests
        run: python3 -m unittest scripts.tests.test_update_homebrew_formula -v
```

- [ ] **Step 6: Commit the tested updater**

```shell
git add scripts/update_homebrew_formula.py scripts/tests/test_update_homebrew_formula.py .github/workflows/ci.yml
git commit -m "test: add safe Homebrew formula updater"
```

### Task 2: Idempotent arm64 release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes manual input `tag` or the pushed tag name.
- Consumes encrypted secret `HOMEBREW_TAP_DEPLOY_KEY` only in the tap checkout step.
- Publishes `codex-router-v<version>-aarch64-apple-darwin.tar.gz`.
- Updates `homebrew-taps/Formula/codex-router.rb` through the Task 1 CLI.

- [ ] **Step 1: Add triggers and least-privilege permissions**

Use `push.tags: ["v*.*.*"]`, a required manual `workflow_dispatch.tag`, `permissions: contents: write`, and per-tag concurrency without cancelling another run.

- [ ] **Step 2: Separate automation source from tagged release source**

Check out the workflow commit into `automation/` and the requested release tag into `release-source/`. This lets a manual retry of historical `v0.1.2` use current safe automation while building the exact historical source.

- [ ] **Step 3: Add release guards and build**

Fail unless the tag matches `^v[0-9]+\.[0-9]+\.[0-9]+$`, `uname -m` is `arm64`, the tag is exact for the checked-out commit, and Cargo metadata reports the same version. Build with:

```shell
cargo build --locked --release -p codex-router-cli --bin codex-router
```

Require the binary to be Mach-O arm64, pass `codesign --verify`, and print exactly the expected CLI version.

- [ ] **Step 4: Package and publish idempotently through REST**

Create the versioned archive under `RUNNER_TEMP`, compute SHA-256, create the release if absent, delete only an existing asset with the exact expected name, and upload the replacement with GitHub's release-upload endpoint. Never enable shell tracing.

- [ ] **Step 5: Update and validate the tap before push**

Check out `ShravanSunder/homebrew-taps` with the deploy key, run the updater, expose that checkout to Homebrew as `shravansunder/taps`, then run:

```shell
ruby -c Formula/codex-router.rb
brew style shravansunder/taps/codex-router
brew audit --strict shravansunder/taps/codex-router
brew install shravansunder/taps/codex-router
brew test shravansunder/taps/codex-router
brew linkage --test codex-router
file "$(brew --prefix codex-router)/bin/codex-router"
```

Require `arm64` and the expected version before any tap commit.

- [ ] **Step 6: Commit only a real formula change**

If `Formula/codex-router.rb` differs, commit only that path as `github-actions[bot]` with message `codex-router <version>` and push `HEAD:main`. If it is identical, report an idempotent no-op.

- [ ] **Step 7: Validate workflow syntax**

Run Ruby YAML parsing and repo-local/actionlint validation. Expected: YAML parses, actionlint reports zero errors, and existing workflow lint remains configured.

- [ ] **Step 8: Commit the workflow**

```shell
git add .github/workflows/release.yml
git commit -m "ci: autopublish Homebrew release"
```

### Task 3: Provision and verify the single-repository deploy key

**Files:**
- No repository files contain key material.

**Interfaces:**
- GitHub deploy key title: `codex-router Homebrew publisher`
- Actions secret name: `HOMEBREW_TAP_DEPLOY_KEY`

- [ ] **Step 1: Confirm no conflicting credential exists**

Read only the deploy-key titles/IDs on `homebrew-taps` and secret names on `codex-router`. Do not request or display key bodies or secret values.

- [ ] **Step 2: Generate temporary key material privately**

Create an owner-only directory with `mktemp -d` under the system temporary directory, set `umask 077`, and generate an Ed25519 key without a passphrase or printed private material.

- [ ] **Step 3: Install both credential ends**

Add the public key to `homebrew-taps` as a write-enabled deploy key. Pipe the private key directly into `gh secret set HOMEBREW_TAP_DEPLOY_KEY --repo ShravanSunder/codex-router`.

- [ ] **Step 4: Delete temporary key material and verify metadata**

Delete the two temporary files, remove the now-empty temporary directory, confirm the secret name exists, and confirm the deploy key is verified, write-enabled, and restricted to `homebrew-taps`.

### Task 4: Publish and prove the real workflow

**Files:**
- Modify through automation: `/Users/shravansunder/Documents/dev/open-source/ai-dev/homebrew-taps/Formula/codex-router.rb`

**Interfaces:**
- Manual proof input: `tag=v0.1.2`
- Expected release source commit: `77c32a920f8c547cd05f19558d61bd94c0ad27c5`

- [ ] **Step 1: Run all local gates**

Run updater tests, `cargo fmt --all -- --check`, workflow YAML parsing, actionlint, `git diff --check`, and verify only planned files plus the accepted spec/plan commits differ from `origin/main`.

- [ ] **Step 2: Push current main without touching the unrelated untracked spec**

Push the implementation commits to `origin/main`. Do not add, edit, or delete `docs/specs/2026-07-12-shared-codex-app-server-host.md`.

- [ ] **Step 3: Dispatch and watch the historical release proof**

Dispatch `release.yml` on `main` with `tag=v0.1.2`, resolve the run ID through REST, and watch it with:

```shell
gh run watch <run-id> --exit-status --interval 45
```

- [ ] **Step 4: Verify external state independently**

Use REST to prove the workflow conclusion and head SHA, release asset name/digest/state, deploy-key metadata, tap commit/file content, and CI checks. Pull the tap checkout and run Homebrew style, strict audit, test, linkage, file, code-signature, and version checks locally.

- [ ] **Step 5: Confirm production isolation**

Verify the existing `codex-router serve` PID and start time are unchanged from the pre-execution snapshot. No process-management command is permitted.

- [ ] **Step 6: Record final proof**

Report exact commands, exit codes, test counts, workflow run URL/conclusion, release digest, tap commit, installed version/architecture, secret/deploy-key metadata without values, and any skipped proof layer with reason.
