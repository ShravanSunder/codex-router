#!/usr/bin/env python3
"""Prepare SQLx metadata for crates whose checked queries run on the Turso engine.

Stock SQLite metadata lives in the root `.sqlx/` and belongs to `prepare-sqlx.py`. Turso
metadata lives beside each crate that invokes the Turso macros, in `crates/<package>/.sqlx/`,
which the macros consult before the root cache. For each target this script builds a fresh
native Turso schema from the crate's migrations through the driver itself, describes every
checked query against it with a forced online compile, and either replaces the crate's metadata
(default) or requires the committed files to match exactly (`--check`).
"""

import os
import subprocess
import sys
import tempfile
import typing as t
from pathlib import Path


TursoPreparationTarget = tuple[str, tuple[str, ...]]
TURSO_PREPARATION_TARGETS: t.Final[tuple[TursoPreparationTarget, ...]] = (
    ("sqlx-turso", ("crates/sqlx-turso/tests/migrations",)),
)
SCHEMA_SEED_EXAMPLE: t.Final[str] = "seed_native_schema"


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


def describe_metadata_differences(
    committed: dict[str, bytes], prepared: dict[str, bytes]
) -> list[str]:
    differences = [f"missing {name}" for name in sorted(prepared.keys() - committed.keys())]
    differences += [f"stale {name}" for name in sorted(committed.keys() - prepared.keys())]
    differences += [
        f"changed {name}"
        for name in sorted(committed.keys() & prepared.keys())
        if committed[name] != prepared[name]
    ]
    return differences


def offline_environment() -> dict[str, str]:
    environment = os.environ | {"SQLX_OFFLINE": "true"}
    _ = environment.pop("DATABASE_URL", None)
    _ = environment.pop("SQLX_OFFLINE_DIR", None)
    return environment


def preparation_commands(
    *,
    package_name: str,
    migration_sources: tuple[str, ...],
    schema_database: Path,
    staging_directory: Path,
) -> list[tuple[list[str], dict[str, str]]]:
    offline = offline_environment()
    describing = offline | {
        "SQLX_OFFLINE": "false",
        "DATABASE_URL": f"turso:{schema_database}",
        "SQLX_OFFLINE_DIR": str(staging_directory),
    }
    return [
        (
            [
                "cargo",
                "run",
                "--locked",
                "--package",
                package_name,
                "--example",
                SCHEMA_SEED_EXAMPLE,
                "--",
                str(schema_database),
                *migration_sources,
            ],
            offline,
        ),
        # Proc macros re-run only when their crate recompiles, and Cargo does not track the
        # macros' environment, so the package's own artifacts are cleaned first.
        (["cargo", "clean", "--package", package_name], offline),
        # One rustc at a time: Turso locks the schema file per process.
        (
            [
                "cargo",
                "check",
                "--locked",
                "--jobs",
                "1",
                "--package",
                package_name,
                "--all-targets",
                "--all-features",
            ],
            describing,
        ),
    ]


def prepare_turso_metadata(
    *,
    repository_root: Path,
    check_metadata: bool,
    run_process: ProcessRunner = subprocess.run,
    targets: tuple[TursoPreparationTarget, ...] = TURSO_PREPARATION_TARGETS,
) -> int:
    temporary_root = repository_root / "tmp"
    temporary_root.mkdir(exist_ok=True)
    try:
        for package_name, migration_sources in targets:
            metadata_directory = repository_root / "crates" / package_name / ".sqlx"
            with tempfile.TemporaryDirectory(
                prefix="sqlx-turso-schema-", dir=temporary_root
            ) as directory:
                schema_database = Path(directory) / "schema.db"
                staging_directory = Path(directory) / "metadata"
                staging_directory.mkdir()
                for command, environment in preparation_commands(
                    package_name=package_name,
                    migration_sources=migration_sources,
                    schema_database=schema_database,
                    staging_directory=staging_directory,
                ):
                    result = run_process(
                        command, cwd=repository_root, env=environment, check=False
                    )
                    if result.returncode:
                        return result.returncode
                prepared = read_query_metadata(staging_directory)
            if not prepared:
                print(
                    f"{package_name}: no checked queries were described",
                    file=sys.stderr,
                )
                return 1
            committed = read_query_metadata(metadata_directory)
            if check_metadata:
                differences = describe_metadata_differences(committed, prepared)
                if differences:
                    print(
                        f"{package_name}: Turso SQLx metadata is out of date; run "
                        "python3 scripts/tooling/prepare-sqlx-turso.py",
                        file=sys.stderr,
                    )
                    for difference in differences:
                        print(f"  {difference}", file=sys.stderr)
                    return 1
            else:
                replace_query_metadata(metadata_directory, prepared)
    except OSError as error:
        print(f"Turso SQLx metadata preparation failed: {error}", file=sys.stderr)
        return 1
    return 0


def main() -> int:
    if sys.argv[1:] not in ([], ["--check"]):
        print("usage: prepare-sqlx-turso.py [--check]", file=sys.stderr)
        return 2
    return prepare_turso_metadata(
        repository_root=Path(__file__).resolve().parents[2],
        check_metadata=sys.argv[1:] == ["--check"],
    )


if __name__ == "__main__":
    raise SystemExit(main())
