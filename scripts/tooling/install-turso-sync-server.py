#!/usr/bin/env python3
"""Install the pinned `tursodb` sync server that the sqlx-turso Sync tests run against.

The Turso sync server ships only inside the Turso CLI release, not as a library crate, so the
tests use the release binary. This script downloads `turso_cli-<target>.tar.xz` for the pinned
version, refuses it unless its SHA-256 matches the digest pinned below, extracts only the
`tursodb` member, refuses it unless its own SHA-256 matches the binary digest pinned below,
installs it atomically into `tmp/rust-tools/bin/` (or `$ROUTER_TOOL_INSTALL_ROOT/bin/`), and
verifies `tursodb --version`. An existing binary, such as one restored from a CI cache, is kept
only when its SHA-256 matches the pin and it reports the pinned version. `--check` verifies an
existing installation that way without downloading.
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
# Digests of the `tursodb` binary inside each archive, computed from the verified archives.
BINARY_SHA256: t.Final[dict[str, str]] = {
    "aarch64-apple-darwin": "fe4e14355e966207922561a602964cfc82328aebeaa6db6b84e0628da1e0d97b",
    "x86_64-apple-darwin": "8d539d21cea1d8b9abb4893ff83050eb6f24cca55e5beb197ed4956bc03ea312",
    "aarch64-unknown-linux-gnu": "90bad14fcb5cfdea84cfb80e87342c5262427fc4b0cd98032be1778bfb7cc3f4",
    "x86_64-unknown-linux-gnu": "57f21919a4bce47aff3f780b2269e4785f01c9469d452144a2508486a6766890",
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
        content = extracted.read()
    binary_digest = hashlib.sha256(content).hexdigest()
    if binary_digest != BINARY_SHA256[target]:
        raise SyncServerInstallError(
            f"{member_name} has SHA-256 {binary_digest}, expected {BINARY_SHA256[target]}"
        )
    return content


def installed_binary_matches(binary: Path, target: str, probe_version: VersionProbe) -> bool:
    try:
        digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    except OSError:
        return False
    return digest == BINARY_SHA256[target] and probe_version(binary) == EXPECTED_VERSION_OUTPUT


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
    if installed_binary_matches(binary, target, probe_version):
        print(f"tursodb {TURSO_VERSION} at {binary}")
        return 0
    if check_only:
        print(
            f"missing tursodb at {binary}, or its SHA-256 or version does not match the pin; "
            "run python3 scripts/tooling/install-turso-sync-server.py",
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

    if not installed_binary_matches(binary, target, probe_version):
        print(
            f"installed tursodb reports {probe_version(binary)!r} or a different SHA-256; "
            f"expected {EXPECTED_VERSION_OUTPUT!r} and {BINARY_SHA256[target]}",
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
