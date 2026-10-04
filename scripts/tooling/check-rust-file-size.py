#!/usr/bin/env python3
"""Reject repository Rust sources over 1000 physical lines, including new files."""

import json
import os
import subprocess
import sys
import typing as t
from pathlib import Path


MAXIMUM_PHYSICAL_LINES: t.Final[int] = 1000


class RustFileSizeCheckError(Exception):
    """The source inventory could not be checked completely."""


def run_git_command(arguments: list[str], directory: Path, failure: str) -> bytes:
    try:
        result = subprocess.run(
            ["git", *arguments],
            cwd=directory,
            capture_output=True,
            check=False,
        )
    except OSError as error:
        raise RustFileSizeCheckError(f"{failure}: {error}") from error
    if result.returncode:
        detail = result.stderr.decode(errors="replace").strip()
        raise RustFileSizeCheckError(f"{failure}: {detail}")
    return result.stdout


def find_repository_root() -> Path:
    output = run_git_command(
        ["rev-parse", "--show-toplevel"],
        Path.cwd(),
        "cannot discover Git repository",
    )
    return Path(os.fsdecode(output.removesuffix(b"\n")))


def find_rust_source_paths(repository_root: Path) -> list[str]:
    output = run_git_command(
        ["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.rs"],
        repository_root,
        "cannot enumerate Rust sources",
    )
    return sorted({os.fsdecode(path) for path in output.split(b"\0") if path})


def count_physical_lines(contents: bytes) -> int:
    return contents.count(b"\n") + int(bool(contents) and not contents.endswith(b"\n"))


def main() -> int:
    if len(sys.argv) != 1:
        print("Usage: python3 scripts/tooling/check-rust-file-size.py", file=sys.stderr)
        return 2

    try:
        repository_root = find_repository_root()
        source_paths = find_rust_source_paths(repository_root)
        violations: list[tuple[str, int]] = []
        for source_path in source_paths:
            try:
                contents = (repository_root / source_path).read_bytes()
            except OSError as error:
                raise RustFileSizeCheckError(
                    f"cannot read Rust source {json.dumps(source_path)}: {error}"
                ) from error
            physical_lines = count_physical_lines(contents)
            if physical_lines > MAXIMUM_PHYSICAL_LINES:
                violations.append((source_path, physical_lines))
    except RustFileSizeCheckError as error:
        print(f"Rust file size check failed: {error}", file=sys.stderr)
        return 2

    for source_path, physical_lines in violations:
        print(
            f"Rust file size error: {json.dumps(source_path)} has {physical_lines} "
            f"physical lines (maximum {MAXIMUM_PHYSICAL_LINES}).",
            file=sys.stderr,
        )
    if violations:
        return 1
    print(
        f"Rust file size check passed: {len(source_paths)} files "
        f"(maximum {MAXIMUM_PHYSICAL_LINES} physical lines)."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
