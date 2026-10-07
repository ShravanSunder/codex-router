#!/usr/bin/env python3
"""Install the pinned `tursodb` sync server that the sqlx-turso Sync tests run against.

The Turso sync server ships only inside the Turso CLI release, not as a library crate, so the
tests use the release binary. This script downloads `turso_cli-<target>.tar.xz` for the pinned
version, refuses it unless its SHA-256 matches the digest pinned below, extracts only the
`tursodb` member, installs it atomically into `tmp/rust-tools/bin/` (or
`$ROUTER_TOOL_INSTALL_ROOT/bin/`), and verifies `tursodb --version`. `--check` verifies an
existing installation without downloading.
"""

import hashlib
import io
import os
import platform
import subprocess
import sys
import tarfile
import tempfile
import typing as t
import urllib.request
from pathlib import Path


TURSO_VERSION: t.Final[str] = "0.8.1"
EXPECTED_VERSION_OUTPUT: t.Final[str] = f"Turso {TURSO_VERSION}"
RELEASE_URL: t.Final[str] = (
    "https://github.com/tursodatabase/turso/releases/download/v{version}/{archive}"
)
# Digests of the v0.8.1 release archives, from the release's own `.sha256` files.
ARCHIVE_SHA256: t.Final[dict[str, str]] = {
    "aarch64-apple-darwin": "0c007da68b556e1e0d9cf8f78fedd6a68ca9b052e603f538071f876077aba5f2",
    "x86_64-apple-darwin": "bfb324858bb1d3d5f609f87f421d10aebfbae22039c8854b700b7e07aa156523",
    "aarch64-unknown-linux-gnu": "a1dfe53b18e273beb97e91f32e57148692e0529450d0db85234a524b5dfafd37",
    "x86_64-unknown-linux-gnu": "b4b94f334cc8ccbf6a7cde1aa7c2949acbc51c83dc41680192a47bd619c021eb",
}
DOWNLOAD_TIMEOUT_SECONDS: t.Final[int] = 300


class SyncServerInstallError(Exception):
    """The sync server could not be installed or verified."""


Fetcher = t.Callable[[str], bytes]
VersionProbe = t.Callable[[Path], str | None]


def host_target(system: str, machine: str) -> str:
    architecture = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "amd64": "x86_64"}
    platform_suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}
    resolved_architecture = architecture.get(machine.lower())
    resolved_platform = platform_suffix.get(system)
    if resolved_architecture is None or resolved_platform is None:
        raise SyncServerInstallError(f"no pinned tursodb build for {system} {machine}")
    return f"{resolved_architecture}-{resolved_platform}"


def archive_name(target: str) -> str:
    return f"turso_cli-{target}.tar.xz"


def fetch_url(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=DOWNLOAD_TIMEOUT_SECONDS) as response:
        return t.cast(bytes, response.read())


def probe_installed_version(binary: Path) -> str | None:
    if not binary.is_file():
        return None
    try:
        result = subprocess.run(
            [str(binary), "--version"], capture_output=True, check=False, timeout=60
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode:
        return None
    return result.stdout.decode(errors="replace").strip()


def verified_tursodb_bytes(archive: bytes, target: str) -> bytes:
    digest = hashlib.sha256(archive).hexdigest()
    expected = ARCHIVE_SHA256[target]
    if digest != expected:
        raise SyncServerInstallError(
            f"{archive_name(target)} has SHA-256 {digest}, expected {expected}"
        )
    member_name = f"turso_cli-{target}/tursodb"
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:xz") as bundle:
        try:
            member = bundle.getmember(member_name)
        except KeyError as error:
            raise SyncServerInstallError(f"{member_name} is not in the archive") from error
        if not member.isfile():
            raise SyncServerInstallError(f"{member_name} is not a regular file")
        extracted = bundle.extractfile(member)
        if extracted is None:
            raise SyncServerInstallError(f"cannot read {member_name}")
        return extracted.read()


def install_binary(binary: Path, content: bytes) -> None:
    binary.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=binary.parent, prefix=".tursodb-", delete=False) as staged:
        staged_path = Path(staged.name)
        staged.write(content)
    try:
        staged_path.chmod(0o755)
        os.replace(staged_path, binary)
    except OSError:
        staged_path.unlink(missing_ok=True)
        raise


def install_sync_server(
    *,
    install_root: Path,
    target: str,
    check_only: bool,
    fetch: Fetcher = fetch_url,
    probe_version: VersionProbe = probe_installed_version,
) -> int:
    binary = install_root / "bin" / "tursodb"
    if probe_version(binary) == EXPECTED_VERSION_OUTPUT:
        print(f"tursodb {TURSO_VERSION} at {binary}")
        return 0
    if check_only:
        print(
            f"missing or mismatched tursodb at {binary}; run "
            "python3 scripts/tooling/install-turso-sync-server.py",
            file=sys.stderr,
        )
        return 2

    url = RELEASE_URL.format(version=TURSO_VERSION, archive=archive_name(target))
    try:
        content = verified_tursodb_bytes(fetch(url), target)
        install_binary(binary, content)
    except (OSError, tarfile.TarError, SyncServerInstallError) as error:
        print(f"tursodb installation failed: {error}", file=sys.stderr)
        return 1

    installed = probe_version(binary)
    if installed != EXPECTED_VERSION_OUTPUT:
        print(
            f"installed tursodb reports {installed!r}, expected {EXPECTED_VERSION_OUTPUT!r}",
            file=sys.stderr,
        )
        return 1
    print(f"tursodb {TURSO_VERSION} installed at {binary}")
    return 0


def main() -> int:
    if sys.argv[1:] not in ([], ["--check"]):
        print("usage: install-turso-sync-server.py [--check]", file=sys.stderr)
        return 2
    repository_root = Path(__file__).resolve().parents[2]
    install_root = Path(
        os.environ.get("ROUTER_TOOL_INSTALL_ROOT", repository_root / "tmp/rust-tools")
    )
    try:
        target = host_target(platform.system(), platform.machine())
    except SyncServerInstallError as error:
        print(error, file=sys.stderr)
        return 2
    return install_sync_server(
        install_root=install_root,
        target=target,
        check_only=sys.argv[1:] == ["--check"],
    )


if __name__ == "__main__":
    raise SystemExit(main())
