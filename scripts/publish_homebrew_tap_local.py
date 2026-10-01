#!/usr/bin/env python3
"""Publish a verified codex-router release to the Homebrew tap from this Mac.

The release workflow's tap job waits for a second macOS runner only to update the
formula and prove the install. When the release asset is already published, this
script does the same work locally: it verifies the tagged commit and the asset's
digest, updates the formula, runs the tap job's Homebrew checks against a real
install, and pushes the formula. The CI tap job then finds the formula already
matching the release and exits without changes.

It never restarts the running production Router; installing a binary does not
replace the running process.

Run from the repository root: `python3 -m scripts.publish_homebrew_tap_local`.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from scripts.update_homebrew_formula import update_formula_file


REPOSITORY = "ShravanSunder/codex-router"
TAP_NAME = "shravansunder/taps"
FORMULA_NAME = f"{TAP_NAME}/codex-router"
FORMULA_RELATIVE_PATH = Path("Formula/codex-router.rb")
INSTALLED_EXECUTABLES = ("codex-router", "agent-collaboration", "agent-sessions")
WORKSPACE_VERSION_PATTERN = re.compile(
    r'^\[workspace\.package\]\s*$(?:\n(?!\[).*)*?\nversion = "([0-9]+\.[0-9]+\.[0-9]+)"',
    re.MULTILINE,
)
DIGEST_PATTERN = re.compile(r"sha256:([0-9a-f]{64})")


class PublishError(RuntimeError):
    """Raised when the release or the tap is not in a publishable state."""


def workspace_version(cargo_toml_text: str) -> str:
    """Return the workspace package version from the root Cargo.toml text."""
    match = WORKSPACE_VERSION_PATTERN.search(cargo_toml_text)
    if match is None:
        raise PublishError("Cargo.toml has no [workspace.package] version")
    return match.group(1)


def release_asset_name(version: str) -> str:
    """Return the release asset the formula installs for one version."""
    return f"codex-router-v{version}-aarch64-apple-darwin.tar.gz"


def published_asset_sha256(release: dict[str, object], asset_name: str) -> str:
    """Return the sha256 GitHub recorded for the named release asset."""
    assets = release.get("assets")
    if not isinstance(assets, list):
        raise PublishError("release response has no assets")
    for asset in assets:
        if isinstance(asset, dict) and asset.get("name") == asset_name:
            digest = asset.get("digest")
            match = DIGEST_PATTERN.fullmatch(digest) if isinstance(digest, str) else None
            if match is None:
                raise PublishError(f"{asset_name} has no sha256 digest")
            return match.group(1)
    raise PublishError(f"release has no asset named {asset_name}")


def run(command: list[str], *, cwd: Path | None = None) -> str:
    """Run one command, echoing it, and return its stdout; fail on a non-zero exit."""
    print(f"$ {' '.join(command)}", flush=True)
    result = subprocess.run(command, cwd=cwd, check=False, capture_output=True, text=True)
    if result.returncode != 0:
        raise PublishError(
            f"{' '.join(command)} exited {result.returncode}: {result.stderr.strip()}"
        )
    return result.stdout.strip()


def verify_tagged_commit(*, repository_root: Path, tag: str) -> str:
    """Return the tag's commit after proving it is on origin/main."""
    _ = run(["git", "fetch", "--quiet", "--tags", "origin"], cwd=repository_root)
    commit = run(["git", "rev-parse", f"{tag}^{{commit}}"], cwd=repository_root)
    merged = subprocess.run(
        ["git", "merge-base", "--is-ancestor", commit, "origin/main"],
        cwd=repository_root,
        check=False,
    )
    if merged.returncode != 0:
        raise PublishError(f"{tag} ({commit}) is not on origin/main")
    return commit


def download_verified_asset(*, tag: str, asset_name: str, directory: Path) -> str:
    """Download the release asset and return its sha256 after matching GitHub's digest."""
    release = json.loads(run(["gh", "api", f"repos/{REPOSITORY}/releases/tags/{tag}"]))
    expected = published_asset_sha256(release, asset_name)
    _ = run(
        [
            "gh",
            "release",
            "download",
            tag,
            "--repo",
            REPOSITORY,
            "--pattern",
            asset_name,
            "--dir",
            str(directory),
        ]
    )
    actual = hashlib.sha256((directory / asset_name).read_bytes()).hexdigest()
    if actual != expected:
        raise PublishError(f"{asset_name} sha256 {actual} does not match release {expected}")
    return actual


def prepare_tap(tap_path: Path) -> None:
    """Require a clean tap checkout exactly at origin/main."""
    if run(["git", "status", "--porcelain"], cwd=tap_path):
        raise PublishError(f"tap checkout {tap_path} has uncommitted changes")
    _ = run(["git", "fetch", "--quiet", "origin"], cwd=tap_path)
    _ = run(["git", "merge", "--ff-only", "--quiet", "origin/main"], cwd=tap_path)
    # A checkout ahead of origin would let an earlier unpushed commit pass as
    # "already published", or ride along on the push.
    if run(["git", "rev-parse", "HEAD"], cwd=tap_path) != run(
        ["git", "rev-parse", "origin/main"], cwd=tap_path
    ):
        raise PublishError(f"tap checkout {tap_path} is not at origin/main")


def install_action(installed_versions: str, version: str) -> str:
    """Return the brew command that installs `version` fresh, as CI does."""
    if not installed_versions:
        return "install"
    # `upgrade` to an already-installed version is a no-op that would skip the
    # install proof, so a same-version re-publish reinstalls.
    if version in installed_versions.split()[1:]:
        return "reinstall"
    return "upgrade"


def validate_installation(version: str) -> None:
    """Run the release workflow's formula and installation checks on this Mac."""
    environment = {**os.environ, "HOMEBREW_NO_AUTO_UPDATE": "1"}
    _ = run(["brew", "style", FORMULA_NAME])
    _ = run(["brew", "audit", "--strict", FORMULA_NAME])
    installed = subprocess.run(
        ["brew", "list", "--versions", "codex-router"],
        check=False,
        capture_output=True,
        text=True,
    )
    action = install_action(installed.stdout.strip() if installed.returncode == 0 else "", version)
    print(f"$ brew {action} {FORMULA_NAME}", flush=True)
    result = subprocess.run(["brew", action, FORMULA_NAME], env=environment, check=False)
    if result.returncode != 0:
        raise PublishError(f"brew {action} exited {result.returncode}")
    _ = run(["brew", "test", FORMULA_NAME])
    _ = run(["brew", "linkage", "--test", "codex-router"])
    prefix = Path(run(["brew", "--prefix", "codex-router"]))
    binary = prefix / "bin" / "codex-router"
    if "Mach-O 64-bit executable arm64" not in run(["file", str(binary)]):
        raise PublishError("Homebrew installed a non-arm64 binary")
    _ = run(["codesign", "--verify", "--verbose=2", str(binary)])
    for executable in INSTALLED_EXECUTABLES:
        reported = run([str(prefix / "bin" / executable), "--version"])
        if reported != f"{executable} {version}":
            raise PublishError(f"Homebrew installed {reported!r}, expected {executable} {version}")


def commit_and_push_formula(*, tap_path: Path, version: str, push: bool) -> str:
    """Commit only the formula and push it; return what happened."""
    _ = run(["git", "add", "--", str(FORMULA_RELATIVE_PATH)], cwd=tap_path)
    staged = run(["git", "diff", "--cached", "--name-only"], cwd=tap_path)
    if not staged:
        # The checkout is at origin/main, so the published formula already matches.
        return "already published"
    if staged != str(FORMULA_RELATIVE_PATH):
        raise PublishError(f"tap update staged unexpected paths: {staged}")
    _ = run(["git", "commit", "--quiet", "-m", f"codex-router {version}"], cwd=tap_path)
    if not push:
        return "committed locally, not pushed (--no-push)"
    pushed = subprocess.run(
        ["git", "push", "--quiet", "origin", "HEAD:main"], cwd=tap_path, check=False
    )
    if pushed.returncode == 0:
        return "pushed"
    # The CI tap job may have pushed the same release first. Its formula content
    # is deterministic, so identical content on origin means it is published.
    _ = run(["git", "fetch", "--quiet", "origin"], cwd=tap_path)
    published = run(["git", "show", f"origin/main:{FORMULA_RELATIVE_PATH}"], cwd=tap_path)
    local = (tap_path / FORMULA_RELATIVE_PATH).read_text(encoding="utf-8").strip()
    if published != local:
        raise PublishError("push rejected and origin/main holds a different formula")
    # Drop the redundant local commit; index and file already equal origin/main.
    _ = run(["git", "reset", "--soft", "--quiet", "origin/main"], cwd=tap_path)
    return "already published by CI"


def restore_formula(tap_path: Path) -> None:
    """Undo an unvalidated formula edit so the live tap never keeps it."""
    restored = subprocess.run(
        ["git", "checkout", "--", str(FORMULA_RELATIVE_PATH)], cwd=tap_path, check=False
    )
    if restored.returncode != 0:
        print(f"could not restore {tap_path / FORMULA_RELATIVE_PATH}", file=sys.stderr)


class PublishArguments(argparse.Namespace):
    version: str | None = None
    push: bool = True


def parse_arguments() -> PublishArguments:
    """Parse publish arguments."""
    parser = argparse.ArgumentParser(
        description="Publish a verified codex-router release to the Homebrew tap from this Mac."
    )
    _ = parser.add_argument("--version", help="release version; defaults to the workspace version")
    _ = parser.add_argument(
        "--no-push", dest="push", action="store_false", help="validate and commit only"
    )
    return parser.parse_args(namespace=PublishArguments())


def main() -> int:
    """Verify, install, and publish one release to the Homebrew tap."""
    arguments = parse_arguments()
    repository_root = Path(__file__).resolve().parent.parent
    tap_path: Path | None = None
    formula_written = False
    try:
        version = arguments.version or workspace_version(
            (repository_root / "Cargo.toml").read_text(encoding="utf-8")
        )
        tag = f"v{version}"
        # The checks resolve the formula through Homebrew's own tap, so publishing
        # from that same checkout keeps the validated file and the pushed file one.
        tap_path = Path(run(["brew", "--repository", TAP_NAME]))
        commit = verify_tagged_commit(repository_root=repository_root, tag=tag)
        with tempfile.TemporaryDirectory(prefix="codex-router-release-") as directory:
            sha256 = download_verified_asset(
                tag=tag, asset_name=release_asset_name(version), directory=Path(directory)
            )
        prepare_tap(tap_path)
        formula_written = update_formula_file(
            formula_path=tap_path / FORMULA_RELATIVE_PATH,
            version=version,
            sha256=sha256,
            source_commit=commit,
        )
        _ = run(["ruby", "-c", str(tap_path / FORMULA_RELATIVE_PATH)])
        validate_installation(version)
        outcome = commit_and_push_formula(
            tap_path=tap_path, version=version, push=arguments.push
        )
    except (PublishError, OSError, ValueError) as error:
        if formula_written and tap_path is not None:
            restore_formula(tap_path)
        print(f"publish failed: {error}", file=sys.stderr)
        return 1
    print(f"codex-router {version}: {outcome}; validated install on this Mac.")
    print("The running production Router is unchanged; restarting it is a separate decision.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
