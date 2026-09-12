#!/usr/bin/env python3
"""Prepare SQLx metadata from each package's native migrations."""

import os
import subprocess
import sys
import tempfile
import typing as t
from pathlib import Path


SqlxPreparationTarget = tuple[str, str, str]
SQLX_PREPARATION_TARGETS: t.Final[tuple[SqlxPreparationTarget, ...]] = (
    (
        "codex-router-state",
        "crates/codex-router-state/migrations",
        "account-schema.sqlite",
    ),
    (
        "project-board-storage",
        "crates/project-board-storage/migrations",
        "project-board-schema.sqlite",
    ),
)


class ProcessRunner(t.Protocol):
    def __call__(
        self,
        command: list[str],
        *,
        cwd: Path,
        env: dict[str, str] | None = None,
        check: bool,
    ) -> subprocess.CompletedProcess[bytes]: ...


def read_query_metadata(metadata_directory: Path) -> dict[str, bytes]:
    if not metadata_directory.is_dir():
        return {}
    return {
        metadata_path.name: metadata_path.read_bytes()
        for metadata_path in sorted(metadata_directory.glob("query-*.json"))
    }


def replace_query_metadata(
    metadata_directory: Path, query_metadata: dict[str, bytes]
) -> None:
    metadata_directory.mkdir(exist_ok=True)
    for metadata_path in metadata_directory.glob("query-*.json"):
        metadata_path.unlink()
    for metadata_name, metadata_content in sorted(query_metadata.items()):
        (metadata_directory / metadata_name).write_bytes(metadata_content)


def merge_query_metadata(
    combined_metadata: dict[str, bytes],
    package_metadata: dict[str, bytes],
    *,
    package_name: str,
) -> None:
    for metadata_name, metadata_content in package_metadata.items():
        existing_content = combined_metadata.get(metadata_name)
        if existing_content is not None and existing_content != metadata_content:
            raise ValueError(
                f"{package_name} produced conflicting SQLx metadata for {metadata_name}"
            )
        combined_metadata[metadata_name] = metadata_content


def prepare_sqlx_metadata(
    *,
    repository_root: Path,
    check_metadata: bool,
    run_process: ProcessRunner = subprocess.run,
) -> int:
    tool_root = repository_root / "tmp/rust-tools/bin"
    sqlx = tool_root / "sqlx"
    metadata_directory = repository_root / ".sqlx"
    temporary_root = repository_root / "tmp"
    temporary_root.mkdir(exist_ok=True)
    original_metadata = read_query_metadata(metadata_directory)
    combined_metadata: dict[str, bytes] = {}
    preparation_completed = check_metadata

    try:
        with tempfile.TemporaryDirectory(
            prefix="sqlx-schema-", dir=temporary_root
        ) as directory:
            for (
                package_name,
                migration_source,
                database_filename,
            ) in SQLX_PREPARATION_TARGETS:
                database_path = Path(directory) / database_filename
                environment = os.environ | {
                    "DATABASE_URL": f"sqlite://{database_path}",
                    "SQLX_OFFLINE": "false",
                    "PATH": f"{tool_root}:{os.environ.get('PATH', '')}",
                }
                _ = environment.pop("SQLX_OFFLINE_DIR", None)
                prepare_command = ["cargo", "sqlx", "prepare", "--workspace"]
                if check_metadata:
                    prepare_command.append("--check")
                prepare_command.extend(
                    ["--", "--locked", "--package", package_name, "--all-targets"]
                )
                commands = [
                    [str(sqlx), "database", "create"],
                    [
                        str(sqlx),
                        "migrate",
                        "run",
                        "--source",
                        migration_source,
                    ],
                    prepare_command,
                ]
                for command in commands:
                    result = run_process(
                        command,
                        cwd=repository_root,
                        env=environment,
                        check=False,
                    )
                    if result.returncode:
                        return result.returncode
                if not check_metadata:
                    merge_query_metadata(
                        combined_metadata,
                        read_query_metadata(metadata_directory),
                        package_name=package_name,
                    )
        if not check_metadata:
            replace_query_metadata(metadata_directory, combined_metadata)
            preparation_completed = True
    except (OSError, ValueError) as error:
        print(f"SQLx metadata preparation failed: {error}", file=sys.stderr)
        return 1
    finally:
        if not preparation_completed:
            replace_query_metadata(metadata_directory, original_metadata)
    return 0


def main() -> int:
    if sys.argv[1:] not in ([], ["--check"]):
        print("usage: prepare-sqlx.py [--check]", file=sys.stderr)
        return 2
    check_metadata = sys.argv[1:] == ["--check"]
    repository_root = Path(__file__).resolve().parents[2]
    tool_root = repository_root / "tmp/rust-tools/bin"
    sqlx = tool_root / "sqlx"
    if not sqlx.is_file():
        print("run scripts/tooling/bootstrap-tools.sh sqlx first", file=sys.stderr)
        return 2
    verification = subprocess.run(
        [
            str(repository_root / "scripts/tooling/bootstrap-tools.sh"),
            "sqlx",
            "--check",
        ],
        cwd=repository_root,
        check=False,
    )
    if verification.returncode:
        return verification.returncode
    return prepare_sqlx_metadata(
        repository_root=repository_root,
        check_metadata=check_metadata,
    )


if __name__ == "__main__":
    raise SystemExit(main())
