"""Exercise the actual bootstrap process against an isolated installer boundary."""

import os
import subprocess
import tempfile
import typing as t
import unittest
from pathlib import Path

BOOTSTRAP = Path(__file__).resolve().parents[1] / "tooling/bootstrap-tools.sh"


@t.final
class BootstrapToolsTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.binaries = self.root / "fake-bin"
        self.binaries.mkdir()
        self.install_root = self.root / "tools"
        self.versions = self.root / "versions.tsv"
        _ = self.versions.write_text("cargo-nextest 0.9.137 cargo-nextest ci\n")
        self.calls = self.root / "calls"
        installer = self.binaries / "cargo"
        _ = installer.write_text("""#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$INSTALL_CALLS"
[[ "${FAIL_INSTALL:-0}" != 1 ]] || exit 9
while [[ $# -gt 0 ]]; do
 case "$1" in --root) root="$2"; shift 2;; *) shift;; esac
done
mkdir -p "$root/bin"
printf '#!/usr/bin/env bash\\necho "cargo-nextest %s"\\n' "${INSTALLED_VERSION:-0.9.137}" > "$root/bin/cargo-nextest"
chmod +x "$root/bin/cargo-nextest"
""")
        installer.chmod(0o700)
        self.environment = os.environ | {
            "PATH": f"{self.binaries}:{os.environ['PATH']}",
            "ROUTER_TOOL_VERSIONS_FILE": str(self.versions),
            "ROUTER_TOOL_INSTALL_ROOT": str(self.install_root),
            "INSTALL_CALLS": str(self.calls),
        }

    def run_bootstrap(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(BOOTSTRAP), "ci"],
            env=self.environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def seed_version(self, version: str) -> None:
        executable = self.install_root / "bin/cargo-nextest"
        executable.parent.mkdir(parents=True, exist_ok=True)
        _ = executable.write_text(
            f'#!/usr/bin/env bash\n[[ "$1" == nextest ]] || exit 2\necho "cargo-nextest {version}"\n'
        )
        executable.chmod(0o700)

    def test_empty_cache_installs_exact_locked_version(self) -> None:
        result = self.run_bootstrap()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--version =0.9.137 --locked", self.calls.read_text())

    def test_matching_cache_skips_install(self) -> None:
        self.seed_version("0.9.137")
        self.assertEqual(self.run_bootstrap().returncode, 0)
        self.assertFalse(self.calls.exists())

    def test_wrong_cache_is_replaced(self) -> None:
        self.seed_version("0.1.0")
        self.assertEqual(self.run_bootstrap().returncode, 0)
        self.assertTrue(self.calls.exists())

    def test_failed_install_never_uses_wrong_cached_version(self) -> None:
        self.seed_version("0.1.0")
        self.environment["FAIL_INSTALL"] = "1"
        self.assertEqual(self.run_bootstrap().returncode, 9)

    def test_successful_installer_with_wrong_binary_is_rejected(self) -> None:
        self.environment["INSTALLED_VERSION"] = "0.1.0"
        result = self.run_bootstrap()
        self.assertEqual(result.returncode, 1)
        self.assertIn("version verification failed", result.stderr)

    def test_malformed_declaration_cannot_reach_installer(self) -> None:
        _ = self.versions.write_text("cargo-nextest latest cargo-nextest ci\n")
        self.assertEqual(self.run_bootstrap().returncode, 2)
        self.assertFalse(self.calls.exists())

    def test_check_mode_never_installs_missing_tool(self) -> None:
        result = subprocess.run(
            ["bash", str(BOOTSTRAP), "ci", "--check"],
            env=self.environment,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertFalse(self.calls.exists())

    def test_empty_role_cannot_report_success(self) -> None:
        _ = self.versions.write_text("# no declared tools\n")
        self.assertEqual(self.run_bootstrap().returncode, 2)
        self.assertFalse(self.calls.exists())
