#!/usr/bin/env python3
"""Prove checked account SQL using isolated copies of the actual workspace source."""

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


def run_check(
    workspace: Path, environment: dict[str, str]
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "check", "--locked", "-p", "codex-router-state"],
        cwd=workspace,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


def main() -> int:
    repository = Path(__file__).resolve().parents[2]
    evidence = repository / "tmp/rust-reliability-proof/sqlx-contract"
    evidence.mkdir(parents=True, exist_ok=True)
    if not (repository / ".sqlx").is_dir():
        raise RuntimeError(
            "prepare and commit-ready inspect query metadata before contract proof"
        )
    with tempfile.TemporaryDirectory(
        prefix="sqlx-contract-", dir=repository / "tmp"
    ) as temporary:
        workspace = Path(temporary)
        for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
            _ = shutil.copy2(repository / name, workspace / name)
        for name in ("crates", "prototypes", ".cargo", ".sqlx"):
            if (repository / name).exists():
                _ = shutil.copytree(repository / name, workspace / name, symlinks=True)
        environment = os.environ | {
            "SQLX_OFFLINE": "true",
            "CARGO_TARGET_DIR": str(repository / "tmp/sqlx-contract-target"),
        }
        _ = environment.pop("DATABASE_URL", None)
        _ = environment.pop("SQLX_OFFLINE_DIR", None)
        result = run_check(workspace, environment)
        _ = (evidence / "offline-valid.log").write_text(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError(
                "valid offline account build failed; see offline-valid.log"
            )

        metadata = workspace / ".sqlx"
        saved_metadata = workspace / "saved-query-metadata"
        _ = metadata.rename(saved_metadata)
        # Metadata removal alone is not an input Cargo tracks on an already-built crate.
        # Force this compiler probe without discarding ordinary or dependency artifacts.
        cleaning = subprocess.run(
            ["cargo", "clean", "-p", "codex-router-state"],
            cwd=workspace,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        if cleaning.returncode:
            raise RuntimeError(
                "could not invalidate the isolated account compiler probe"
            )
        result = run_check(workspace, environment)
        _ = (evidence / "missing-metadata.log").write_text(
            result.stdout + result.stderr
        )
        if result.returncode == 0 or "cached data" not in result.stderr:
            raise RuntimeError("missing metadata did not fail for the expected reason")
        _ = saved_metadata.rename(metadata)

        sqlx = repository / "tmp/rust-tools/bin/sqlx"
        database = workspace / "proof-schema.sqlite"
        environment["DATABASE_URL"] = f"sqlite://{database}"
        environment["SQLX_OFFLINE"] = "false"
        for command in (
            [str(sqlx), "database", "create"],
            [
                str(sqlx),
                "migrate",
                "run",
                "--source",
                "crates/codex-router-state/migrations",
            ],
        ):
            result = subprocess.run(
                command,
                cwd=workspace,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            if result.returncode:
                raise RuntimeError(
                    "could not create disposable native schema for query proof"
                )

        source_path = workspace / "crates/codex-router-state/src/sqlite.rs"
        if source_path.is_symlink():
            raise RuntimeError("refusing to mutate a symlink in the proof copy")
        original = source_path.read_text()
        start = original.index("pub async fn list_accounts(")
        end = original.index("pub async fn list_account_routing_policies(", start)
        query_section = original[start:end]
        if "sqlx::query!(" not in query_section:
            raise RuntimeError("account list is not a checked SQLx query")
        cases = (
            (
                "invalid-column",
                "SELECT account_id,",
                "SELECT nonexistent_account_column,",
                "no such column",
            ),
            (
                "wrong-result-type",
                "SELECT account_id,",
                'SELECT account_id AS \\"account_id: i64\\",',
                "mismatched types",
            ),
        )
        for label, needle, replacement, expected_error in cases:
            if query_section.count(needle) != 1:
                raise RuntimeError(
                    "account SQL proof anchor changed; update the permanent harness"
                )
            changed = query_section.replace(needle, replacement, 1)
            _ = source_path.write_text(original[:start] + changed + original[end:])
            result = run_check(workspace, environment)
            _ = (evidence / f"{label}.log").write_text(result.stdout + result.stderr)
            if result.returncode == 0 or expected_error not in result.stderr:
                raise RuntimeError(
                    f"{label} did not fail for its expected compiler diagnostic"
                )
        _ = source_path.write_text(original)
        # Freshness is a separate gate from offline compilation: corrupt only
        # the scratch metadata and require comparison with the migrated schema.
        for query_file in sorted(metadata.glob("query-*.json")):
            query_data = json.loads(query_file.read_text())
            nullable = query_data["describe"]["nullable"]
            if nullable:
                nullable[0] = not nullable[0]
                _ = query_file.write_text(json.dumps(query_data))
                break
        else:
            raise RuntimeError("no checked result metadata available for freshness proof")
        environment["PATH"] = f"{sqlx.parent}:{environment['PATH']}"
        result = subprocess.run(
            [
                "cargo", "sqlx", "prepare", "--workspace", "--check",
                "--", "--locked", "--package", "codex-router-state", "--all-targets",
            ],
            cwd=workspace,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        _ = (evidence / "stale-metadata.log").write_text(result.stdout + result.stderr)
        if result.returncode == 0 or "one or more query files differ" not in result.stderr:
            raise RuntimeError("stale metadata did not fail at the freshness comparison")
    print(
        f"SQLx contract proof: valid offline build, three expected compiler failures, "
        f"and stale metadata rejection; logs: {evidence}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
