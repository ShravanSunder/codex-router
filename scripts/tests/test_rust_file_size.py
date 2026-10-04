"""Prove the Rust size gate through real Git inventories and CLI exits."""

import json
import os
import subprocess
import sys
import tempfile
import typing as t
import unittest
from pathlib import Path


CHECKER_PATH: t.Final[Path] = (
    Path(__file__).resolve().parents[1] / "tooling/check-rust-file-size.py"
)


@t.final
class RustFileSizeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repository_root = Path(self.temporary.name)
        self.environment = os.environ | {
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
        }
        self.run_git("init", "--quiet")

    def run_git(self, *arguments: str) -> None:
        subprocess.run(
            ["git", *arguments],
            cwd=self.repository_root,
            env=self.environment,
            capture_output=True,
            check=True,
        )

    def write_source(self, name: str, contents: bytes, *, tracked: bool = False) -> Path:
        source_path = self.repository_root / name
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_bytes(contents)
        if tracked:
            self.run_git("add", "--", name)
        return source_path

    def run_checker(self, *, directory: Path | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(CHECKER_PATH)],
            cwd=directory or self.repository_root,
            env=self.environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def assert_passes(self, result: subprocess.CompletedProcess[str], files: int) -> None:
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        self.assertEqual(
            result.stdout,
            f"Rust file size check passed: {files} files (maximum 1000 physical lines).\n",
        )

    def violation(self, name: str, lines: int) -> str:
        return (
            f"Rust file size error: {json.dumps(name)} has {lines} physical lines "
            "(maximum 1000).\n"
        )

    def test_empty_repository_passes(self) -> None:
        self.assert_passes(self.run_checker(), 0)

    def test_physical_line_boundaries_include_unterminated_tail_and_crlf(self) -> None:
        cases = [
            (b"", 0),
            (b"x", 1),
            (b"x\n", 1),
            (b"\n" * 1000, 1000),
            (b"\n" * 999 + b"tail", 1000),
            (b"\n" * 1000 + b"tail", 1001),
            (b"\r\n" * 1000, 1000),
            (b"\r\n" * 1001, 1001),
        ]
        for contents, lines in cases:
            with self.subTest(lines=lines, length=len(contents)):
                self.write_source("boundary.rs", contents)
                result = self.run_checker()
                if lines <= 1000:
                    self.assert_passes(result, 1)
                else:
                    self.assertEqual(result.returncode, 1)
                    self.assertEqual(result.stdout, "")
                    self.assertEqual(result.stderr, self.violation("boundary.rs", 1001))

    def test_blank_and_comment_lines_are_not_discarded(self) -> None:
        self.write_source("comments.rs", b"// comment\n" * 500 + b"\n" * 501)
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, self.violation("comments.rs", 1001))

    def test_unicode_separators_are_not_physical_lf_lines(self) -> None:
        self.write_source("unicode.rs", ("text\u2028text\u2029" * 1001).encode())
        self.assert_passes(self.run_checker(), 1)

    def test_counts_bytes_without_requiring_utf8_source_contents(self) -> None:
        self.write_source("bytes.rs", b"\xff\n" * 1001)
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, self.violation("bytes.rs", 1001))

    def test_tracked_and_untracked_rust_are_both_checked(self) -> None:
        self.write_source("tracked.rs", b"\n" * 1001, tracked=True)
        self.write_source("new.rs", b"\n" * 1001)
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertEqual(
            result.stderr,
            self.violation("new.rs", 1001) + self.violation("tracked.rs", 1001),
        )

    def test_modified_tracked_source_uses_current_worktree_bytes(self) -> None:
        self.write_source("modified.rs", b"fn original() {}\n", tracked=True)
        self.write_source("modified.rs", b"\n" * 1001)
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, self.violation("modified.rs", 1001))

    def test_tracked_ignored_rust_remains_included(self) -> None:
        self.write_source("tracked.rs", b"\n" * 1001, tracked=True)
        self.write_source(".gitignore", b"*.rs\n")
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, self.violation("tracked.rs", 1001))

    def test_ignored_untracked_build_artifacts_are_not_source_inventory(self) -> None:
        self.write_source(".gitignore", b"/target/\n/tmp/\n")
        self.write_source("target/generated.rs", b"\n" * 1001)
        self.write_source("tmp/scratch.rs", b"\n" * 1001)
        self.write_source("src/source.rs", b"fn source() {}\n")
        self.assert_passes(self.run_checker(), 1)

    def test_vendor_generated_tests_and_prototypes_have_no_exemptions(self) -> None:
        names = ["vendor/a.rs", "generated/b.rs", "tests/c.rs", "prototypes/d.rs"]
        for name in names:
            self.write_source(name, b"\n" * 1001)
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(
            result.stderr,
            "".join(self.violation(name, 1001) for name in sorted(names)),
        )

    def test_non_rust_files_are_not_checked(self) -> None:
        self.write_source("large.py", b"\n" * 1001)
        self.write_source("source.rs", b"fn source() {}\n")
        self.assert_passes(self.run_checker(), 1)

    def test_names_with_spaces_unicode_newlines_and_shell_syntax_are_literal(self) -> None:
        names = ["space name.rs", "caf\u00e9.rs", "line\nbreak.rs", "$(literal);`name`.rs"]
        for name in names:
            self.write_source(name, b"\n" * 1001)
        result = self.run_checker()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(
            result.stderr,
            "".join(self.violation(name, 1001) for name in sorted(names)),
        )

    def test_checking_from_subdirectory_still_covers_repository(self) -> None:
        self.write_source("root.rs", b"\n" * 1001, tracked=True)
        nested = self.repository_root / "nested/deeper"
        nested.mkdir(parents=True)
        result = self.run_checker(directory=nested)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, self.violation("root.rs", 1001))

    def test_missing_tracked_source_is_an_error_not_a_pass(self) -> None:
        source = self.write_source("missing.rs", b"fn missing() {}\n", tracked=True)
        source.unlink()
        result = self.run_checker()
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertIn('cannot read Rust source "missing.rs"', result.stderr)

    def test_invocation_outside_git_repository_is_an_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_checker(directory=Path(directory))
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertIn("cannot discover Git repository", result.stderr)

    def test_one_tracked_path_is_counted_once(self) -> None:
        self.write_source("tracked.rs", b"fn source() {}\n", tracked=True)
        self.assert_passes(self.run_checker(), 1)


if __name__ == "__main__":
    unittest.main()
