"""Verify the tursodb installer pins the release digest and installs only the server binary."""

import hashlib
import importlib.util
import io
import tarfile
import tempfile
import typing as t
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from types import ModuleType


def load_installer_module() -> ModuleType:
    script_path = Path(__file__).resolve().parents[1] / "tooling/install-turso-sync-server.py"
    module_spec = importlib.util.spec_from_file_location("install_turso_sync_server", script_path)
    if module_spec is None or module_spec.loader is None:
        raise RuntimeError(f"cannot load {script_path}")
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    return module


INSTALLER = load_installer_module()
TARGET = "x86_64-unknown-linux-gnu"
EXPECTED = INSTALLER.EXPECTED_VERSION_OUTPUT


def release_archive(target: str, binary: bytes) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:xz") as bundle:
        for name, content in ((f"turso_cli-{target}/tursodb", binary), ("README.md", b"docs")):
            member = tarfile.TarInfo(name)
            member.size = len(content)
            bundle.addfile(member, io.BytesIO(content))
    return buffer.getvalue()


@t.final
class InstallTursoSyncServerTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.install_root = Path(self.directory.name)
        self.binary = self.install_root / "bin/tursodb"

    def install(
        self,
        *,
        archive: bytes,
        check_only: bool = False,
        reported_version: str = EXPECTED,
    ) -> tuple[int, list[str]]:
        fetched: list[str] = []

        def fake_fetch(url: str) -> bytes:
            fetched.append(url)
            return archive

        def fake_probe(binary: Path) -> str | None:
            return reported_version if binary.is_file() else None

        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            result = INSTALLER.install_sync_server(
                install_root=self.install_root,
                target=TARGET,
                check_only=check_only,
                fetch=fake_fetch,
                probe_version=fake_probe,
            )
        return result, fetched

    def pin_digest_of(self, archive: bytes) -> None:
        original = dict(INSTALLER.ARCHIVE_SHA256)
        INSTALLER.ARCHIVE_SHA256[TARGET] = hashlib.sha256(archive).hexdigest()
        self.addCleanup(INSTALLER.ARCHIVE_SHA256.update, original)

    def pin_binary_digest_of(self, binary: bytes) -> None:
        original = dict(INSTALLER.BINARY_SHA256)
        INSTALLER.BINARY_SHA256[TARGET] = hashlib.sha256(binary).hexdigest()
        self.addCleanup(INSTALLER.BINARY_SHA256.update, original)

    def write_cached_binary(self, content: bytes) -> None:
        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(content)

    def test_maps_hosts_to_release_targets(self) -> None:
        self.assertEqual(INSTALLER.host_target("Darwin", "arm64"), "aarch64-apple-darwin")
        self.assertEqual(INSTALLER.host_target("Linux", "x86_64"), "x86_64-unknown-linux-gnu")
        self.assertEqual(INSTALLER.host_target("Linux", "aarch64"), "aarch64-unknown-linux-gnu")
        with self.assertRaises(INSTALLER.SyncServerInstallError):
            INSTALLER.host_target("Windows", "AMD64")

    def test_installs_only_the_server_binary_from_a_verified_archive(self) -> None:
        archive = release_archive(TARGET, b"server")
        self.pin_digest_of(archive)
        self.pin_binary_digest_of(b"server")

        result, fetched = self.install(archive=archive)

        self.assertEqual(result, 0)
        self.assertEqual(self.binary.read_bytes(), b"server")
        self.assertTrue(self.binary.stat().st_mode & 0o111)
        self.assertEqual(sorted(path.name for path in self.binary.parent.iterdir()), ["tursodb"])
        self.assertEqual(len(fetched), 1)
        self.assertIn(f"v{INSTALLER.TURSO_VERSION}/turso_cli-{TARGET}.tar.xz", fetched[0])

    def test_refuses_an_archive_whose_digest_does_not_match(self) -> None:
        result, _ = self.install(archive=release_archive(TARGET, b"tampered"))

        self.assertEqual(result, 1)
        self.assertFalse(self.binary.exists())

    def test_refuses_a_binary_whose_own_digest_does_not_match(self) -> None:
        archive = release_archive(TARGET, b"server")
        self.pin_digest_of(archive)

        result, _ = self.install(archive=archive)

        self.assertEqual(result, 1)
        self.assertFalse(self.binary.exists())

    def test_check_mode_never_downloads(self) -> None:
        result, fetched = self.install(archive=b"", check_only=True)

        self.assertEqual(result, 2)
        self.assertEqual(fetched, [])

    def test_a_matching_installation_is_kept(self) -> None:
        self.write_cached_binary(b"existing")
        self.pin_binary_digest_of(b"existing")

        result, fetched = self.install(archive=b"")

        self.assertEqual(result, 0)
        self.assertEqual(fetched, [])
        self.assertEqual(self.binary.read_bytes(), b"existing")

    def test_a_cached_binary_with_the_right_version_but_another_digest_is_replaced(self) -> None:
        self.write_cached_binary(b"substituted")
        archive = release_archive(TARGET, b"server")
        self.pin_digest_of(archive)
        self.pin_binary_digest_of(b"server")

        checked, _ = self.install(archive=archive, check_only=True)
        installed, fetched = self.install(archive=archive)

        self.assertEqual(checked, 2)
        self.assertEqual(installed, 0)
        self.assertEqual(len(fetched), 1)
        self.assertEqual(self.binary.read_bytes(), b"server")

    def test_a_binary_reporting_the_wrong_version_fails(self) -> None:
        archive = release_archive(TARGET, b"server")
        self.pin_digest_of(archive)
        self.pin_binary_digest_of(b"server")

        result, _ = self.install(archive=archive, reported_version="Turso 0.8.2")

        self.assertEqual(result, 1)


if __name__ == "__main__":
    unittest.main()
