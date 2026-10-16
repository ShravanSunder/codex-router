"""Qualify signed normal binaries and the Host's existing global Claude selection."""

import hashlib
import os
import subprocess
import typing as t
from pathlib import Path

from pydantic import BaseModel, ConfigDict

from scripts.verification.typed_observations import BinaryQualification, QualifiedBinary


class GlobalClaudeBinding(BaseModel):
    model_config: t.ClassVar[ConfigDict] = ConfigDict(frozen=True, extra="forbid")
    canonicalPath: Path
    sha256: str


def sha256(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def qualify_normal_binaries(qualification: Path) -> dict[str, QualifiedBinary]:
    records = BinaryQualification.model_validate_json(qualification.read_text())
    binaries = {item.target: item for item in records.binaries}
    for name in ["codex-router", "agent-collaboration", "agent-sessions"]:
        item = binaries[name]
        if sha256(Path(item.path)) != item.sha256 or item.testGuardFeatures:
            raise RuntimeError("binary hash or normal-build mismatch")
        _ = subprocess.run(
            ["/usr/bin/codesign", "--verify", "--strict", item.path],
            capture_output=True,
            check=True,
        )
        signature = subprocess.run(
            ["/usr/bin/codesign", "-dv", "--verbose=4", item.path],
            capture_output=True,
            text=True,
            check=True,
        ).stderr
        if (
            f"Identifier=dev.shravansunder.{name}.debug" not in signature
            or "TeamIdentifier=974QD84WVC" not in signature
        ):
            raise RuntimeError("debug signing identity mismatch")
    return binaries


def package_search_path(path: Path) -> bool:
    return any(part in {"node_modules", "_npx", "dlx"} for part in path.parts)


def excluded_claude_target(path: Path) -> bool:
    return any(
        part in {"_npx", "dlx"}
        or part.startswith(("claude-agent-sdk", "@anthropic-ai+claude-agent-sdk"))
        for part in path.parts
    )


def resolve_global_claude(search_path: str | None) -> Path:
    """Mirror global_claude_executable.rs; never execute a candidate or change PATH."""
    filename = "claude.exe" if os.name == "nt" else "claude"
    for entry in (search_path or "").split(os.pathsep):
        directory = Path(entry)
        if not directory.is_absolute() or package_search_path(directory):
            continue
        try:
            canonical_directory = directory.resolve(strict=True)
            if package_search_path(canonical_directory):
                continue
            candidate = (canonical_directory / filename).resolve(strict=True)
            if excluded_claude_target(candidate) or not candidate.is_file():
                continue
            if os.name == "posix" and not os.access(
                candidate, os.X_OK, effective_ids=True
            ):
                continue
            _ = str(candidate).encode("utf-8", errors="strict")
            return candidate
        except (OSError, RuntimeError, UnicodeError):
            continue
    raise RuntimeError("qualified global Claude executable unavailable on Host PATH")


def qualify_global_claude(
    planned: Path, search_path: str | None
) -> GlobalClaudeBinding:
    actual = resolve_global_claude(search_path)
    if planned.resolve(strict=True) != actual:
        raise RuntimeError(
            "planned Claude executable differs from qualified Host PATH selection"
        )
    return GlobalClaudeBinding(canonicalPath=actual, sha256=sha256(actual))
