#!/usr/bin/env python3
"""Prepare account-query metadata from native migrations, never a live database."""

import os
import subprocess
import sys
import tempfile
from pathlib import Path


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
    temporary_root = repository_root / "tmp"
    temporary_root.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="sqlx-schema-", dir=temporary_root
    ) as directory:
        database_path = Path(directory) / "account-schema.sqlite"
        environment = os.environ | {
            "DATABASE_URL": f"sqlite://{database_path}",
            "SQLX_OFFLINE": "false",
            "PATH": f"{tool_root}:{os.environ['PATH']}",
        }
        _ = environment.pop("SQLX_OFFLINE_DIR", None)
        commands = [
            [str(sqlx), "database", "create"],
            [
                str(sqlx),
                "migrate",
                "run",
                "--source",
                "crates/codex-router-state/migrations",
            ],
            ["cargo", "sqlx", "prepare", "--workspace"],
        ]
        if check_metadata:
            commands[-1].append("--check")
        commands[-1].extend(
            ["--", "--locked", "--package", "codex-router-state", "--all-targets"]
        )
        for command in commands:
            result = subprocess.run(
                command, cwd=repository_root, env=environment, check=False
            )
            if result.returncode:
                return result.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
