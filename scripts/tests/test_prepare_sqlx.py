"""Verify independent SQLx schemas produce one lossless workspace cache."""

import importlib.util
import io
import subprocess
import tempfile
import typing as t
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from types import ModuleType


def load_prepare_sqlx_module() -> ModuleType:
    script_path = Path(__file__).resolve().parents[1] / "tooling/prepare-sqlx.py"
    module_spec = importlib.util.spec_from_file_location("prepare_sqlx", script_path)
    if module_spec is None or module_spec.loader is None:
        raise RuntimeError(f"cannot load {script_path}")
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    return module


PREPARE_SQLX = load_prepare_sqlx_module()


@t.final
class PrepareSqlxTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repository_root = Path(self.directory.name)
        self.metadata_directory = self.repository_root / ".sqlx"
        self.metadata_directory.mkdir()
        tool_directory = self.repository_root / "tmp/rust-tools/bin"
        tool_directory.mkdir(parents=True)
        (tool_directory / "sqlx").touch()

    def run_workflow(
        self,
        *,
        check_metadata: bool,
        prepare_failure_package: str | None = None,
        conflicting_metadata: bool = False,
    ) -> tuple[int, list[tuple[list[str], dict[str, str]]]]:
        calls: list[tuple[list[str], dict[str, str]]] = []

        def fake_run(
            command: list[str],
            *,
            cwd: Path,
            env: dict[str, str] | None = None,
            check: bool,
        ) -> subprocess.CompletedProcess[str]:
            self.assertEqual(cwd, self.repository_root)
            self.assertFalse(check)
            captured_environment = dict(env or {})
            calls.append((command, captured_environment))
            if command[:4] == ["cargo", "sqlx", "prepare", "--workspace"]:
                package_index = command.index("--package") + 1
                package_name = command[package_index]
                if not check_metadata:
                    for metadata_path in self.metadata_directory.glob("query-*.json"):
                        metadata_path.unlink()
                    metadata_name = (
                        "query-shared.json"
                        if conflicting_metadata
                        else f"query-{package_name}.json"
                    )
                    (self.metadata_directory / metadata_name).write_text(package_name)
                return subprocess.CompletedProcess(
                    command,
                    7 if package_name == prepare_failure_package else 0,
                )
            return subprocess.CompletedProcess(command, 0)

        result = PREPARE_SQLX.prepare_sqlx_metadata(
            repository_root=self.repository_root,
            check_metadata=check_metadata,
            run_process=fake_run,
        )
        return result, calls

    def test_prepares_each_package_against_only_its_migrations(self) -> None:
        result, calls = self.run_workflow(check_metadata=False)

        self.assertEqual(result, 0)
        migration_calls = [
            (command, environment)
            for command, environment in calls
            if command[1:3] == ["migrate", "run"]
        ]
        prepare_calls = [
            (command, environment)
            for command, environment in calls
            if command[:4] == ["cargo", "sqlx", "prepare", "--workspace"]
        ]
        self.assertEqual(
            [command[3] for command, _ in migration_calls],
            ["--source", "--source"],
        )
        self.assertEqual(
            [command[4] for command, _ in migration_calls],
            [
                "crates/codex-router-state/migrations",
                "crates/project-board-storage/migrations",
            ],
        )
        self.assertEqual(
            [command[command.index("--package") + 1] for command, _ in prepare_calls],
            ["codex-router-state", "project-board-storage"],
        )
        self.assertEqual(
            [
                Path(environment["DATABASE_URL"].removeprefix("sqlite://")).name
                for _, environment in prepare_calls
            ],
            ["account-schema.sqlite", "project-board-schema.sqlite"],
        )

    def test_successful_prepare_publishes_combined_metadata(self) -> None:
        (self.metadata_directory / "query-stale.json").write_text("stale")
        (self.metadata_directory / "README.md").write_text("preserve me")

        result, _ = self.run_workflow(check_metadata=False)

        self.assertEqual(result, 0)
        self.assertEqual(
            sorted(path.name for path in self.metadata_directory.glob("query-*.json")),
            [
                "query-codex-router-state.json",
                "query-project-board-storage.json",
            ],
        )
        self.assertEqual(
            (self.metadata_directory / "README.md").read_text(), "preserve me"
        )

    def test_failed_prepare_restores_original_metadata(self) -> None:
        (self.metadata_directory / "query-original.json").write_text("original")

        result, _ = self.run_workflow(
            check_metadata=False,
            prepare_failure_package="project-board-storage",
        )

        self.assertEqual(result, 7)
        self.assertEqual(
            sorted(path.name for path in self.metadata_directory.glob("query-*.json")),
            ["query-original.json"],
        )
        self.assertEqual(
            (self.metadata_directory / "query-original.json").read_text(), "original"
        )

    def test_conflicting_same_hash_metadata_restores_original_cache(self) -> None:
        (self.metadata_directory / "query-original.json").write_text("original")

        error_output = io.StringIO()
        with redirect_stderr(error_output):
            result, _ = self.run_workflow(
                check_metadata=False,
                conflicting_metadata=True,
            )

        self.assertEqual(result, 1)
        self.assertIn("conflicting SQLx metadata", error_output.getvalue())
        self.assertEqual(
            sorted(path.name for path in self.metadata_directory.glob("query-*.json")),
            ["query-original.json"],
        )

    def test_check_validates_both_packages_without_mutating_metadata(self) -> None:
        existing_metadata = self.metadata_directory / "query-existing.json"
        existing_metadata.write_text("existing")

        result, calls = self.run_workflow(check_metadata=True)

        self.assertEqual(result, 0)
        prepare_calls = [
            command
            for command, _ in calls
            if command[:4] == ["cargo", "sqlx", "prepare", "--workspace"]
        ]
        self.assertEqual(len(prepare_calls), 2)
        self.assertTrue(all("--check" in command for command in prepare_calls))
        self.assertEqual(existing_metadata.read_text(), "existing")


if __name__ == "__main__":
    _ = unittest.main()
