"""Verify Turso metadata is described natively and compared exactly, beside the stock cache."""

import importlib.util
import io
import subprocess
import tempfile
import typing as t
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from types import ModuleType


def load_prepare_sqlx_turso_module() -> ModuleType:
    script_path = Path(__file__).resolve().parents[1] / "tooling/prepare-sqlx-turso.py"
    module_spec = importlib.util.spec_from_file_location("prepare_sqlx_turso", script_path)
    if module_spec is None or module_spec.loader is None:
        raise RuntimeError(f"cannot load {script_path}")
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    return module


PREPARE_SQLX_TURSO = load_prepare_sqlx_turso_module()
TARGETS = (("sqlx-turso", ("crates/sqlx-turso/tests/migrations",)),)


@t.final
class PrepareSqlxTursoTests(unittest.TestCase):
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repository_root = Path(self.directory.name)
        self.metadata_directory = self.repository_root / "crates/sqlx-turso/.sqlx"
        self.metadata_directory.mkdir(parents=True)
        self.stock_metadata = self.repository_root / ".sqlx"
        self.stock_metadata.mkdir()
        (self.stock_metadata / "query-stock.json").write_text("stock")

    def run_workflow(
        self,
        *,
        check_metadata: bool,
        described: dict[str, str] | None = None,
        failing_command: str | None = None,
    ) -> tuple[int, list[tuple[list[str], dict[str, str]]]]:
        calls: list[tuple[list[str], dict[str, str]]] = []
        described_metadata = described if described is not None else {"query-a.json": "a"}

        def fake_run(
            command: list[str],
            *,
            cwd: Path,
            env: dict[str, str] | None = None,
            check: bool,
        ) -> subprocess.CompletedProcess[bytes]:
            self.assertEqual(cwd, self.repository_root)
            self.assertFalse(check)
            environment = dict(env or {})
            calls.append((command, environment))
            if command[1] == failing_command:
                return subprocess.CompletedProcess(command, 9)
            if command[1] == "check":
                staging = Path(environment["SQLX_OFFLINE_DIR"])
                for name, content in described_metadata.items():
                    (staging / name).write_text(content)
            return subprocess.CompletedProcess(command, 0)

        result = PREPARE_SQLX_TURSO.prepare_turso_metadata(
            repository_root=self.repository_root,
            check_metadata=check_metadata,
            run_process=fake_run,
            targets=TARGETS,
        )
        return result, calls

    def test_seeds_a_native_schema_then_describes_online_one_compiler_at_a_time(self) -> None:
        result, calls = self.run_workflow(check_metadata=False)

        self.assertEqual(result, 0)
        self.assertEqual([command[1] for command, _ in calls], ["run", "clean", "check"])
        (seed, seed_environment), (_, _), (check, check_environment) = calls
        self.assertIn("seed_native_schema", seed)
        self.assertEqual(seed[-1], "crates/sqlx-turso/tests/migrations")
        self.assertEqual(seed_environment["SQLX_OFFLINE"], "true")
        self.assertNotIn("DATABASE_URL", seed_environment)
        schema_database = seed[seed.index("--") + 1]
        self.assertEqual(check_environment["DATABASE_URL"], f"turso:{schema_database}")
        self.assertEqual(check_environment["SQLX_OFFLINE"], "false")
        self.assertEqual(check[check.index("--jobs") + 1], "1")
        self.assertIn("--all-features", check)
        self.assertIn("--all-targets", check)

    def test_write_mode_replaces_only_the_crate_metadata(self) -> None:
        (self.metadata_directory / "query-stale.json").write_text("stale")

        result, _ = self.run_workflow(
            check_metadata=False, described={"query-a.json": "a", "query-b.json": "b"}
        )

        self.assertEqual(result, 0)
        self.assertEqual(
            sorted(path.name for path in self.metadata_directory.glob("query-*.json")),
            ["query-a.json", "query-b.json"],
        )
        self.assertEqual((self.stock_metadata / "query-stock.json").read_text(), "stock")

    def test_a_failed_step_leaves_committed_metadata_untouched(self) -> None:
        (self.metadata_directory / "query-original.json").write_text("original")

        result, calls = self.run_workflow(check_metadata=False, failing_command="check")

        self.assertEqual(result, 9)
        self.assertEqual(len(calls), 3)
        self.assertEqual(
            sorted(path.name for path in self.metadata_directory.glob("query-*.json")),
            ["query-original.json"],
        )

    def test_check_passes_only_on_identical_names_and_bytes(self) -> None:
        (self.metadata_directory / "query-a.json").write_text("a")

        result, _ = self.run_workflow(check_metadata=True)

        self.assertEqual(result, 0)

    def test_check_reports_missing_stale_and_changed_files(self) -> None:
        (self.metadata_directory / "query-a.json").write_text("old a")
        (self.metadata_directory / "query-stale.json").write_text("stale")

        error_output = io.StringIO()
        with redirect_stderr(error_output):
            result, _ = self.run_workflow(
                check_metadata=True, described={"query-a.json": "a", "query-new.json": "n"}
            )

        self.assertEqual(result, 1)
        report = error_output.getvalue()
        self.assertIn("missing query-new.json", report)
        self.assertIn("stale query-stale.json", report)
        self.assertIn("changed query-a.json", report)
        self.assertEqual((self.metadata_directory / "query-a.json").read_text(), "old a")

    def test_describing_no_queries_is_an_error(self) -> None:
        error_output = io.StringIO()
        with redirect_stderr(error_output):
            result, _ = self.run_workflow(check_metadata=False, described={})

        self.assertEqual(result, 1)
        self.assertIn("no checked queries", error_output.getvalue())


if __name__ == "__main__":
    unittest.main()
