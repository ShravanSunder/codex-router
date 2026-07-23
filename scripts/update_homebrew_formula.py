#!/usr/bin/env python3

import argparse
import os
from pathlib import Path
import re
import stat
import sys
import tempfile


VERSION_PATTERN = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
SOURCE_COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")
FORMULA_URL_PATTERN = re.compile(
    r'^  url "https://github\.com/ShravanSunder/codex-router/releases/download/'
    r'v[^"]+/codex-router-v[^"]+-aarch64-apple-darwin\.tar\.gz"$',
    re.MULTILINE,
)
FORMULA_SHA256_PATTERN = re.compile(r'^  sha256 "[0-9a-f]{64}"$', re.MULTILINE)
FORMULA_SOURCE_COMMIT_PATTERN = re.compile(
    r"^  # Source commit: [0-9a-f]{40}$",
    re.MULTILINE,
)
REQUIRED_FORMULA_INVARIANTS = (
    ('  depends_on arch: :arm64', "Apple Silicon requirement"),
    ('  depends_on :macos', "macOS requirement"),
    ('    bin.install "codex-router"', "direct binary installation"),
)


class FormulaUpdateError(ValueError):
    """Raised when a requested formula update violates the release contract."""


def _replace_exactly_once(
    *,
    formula_text: str,
    pattern: re.Pattern[str],
    replacement: str,
    field_name: str,
) -> str:
    updated_formula, replacement_count = pattern.subn(replacement, formula_text)
    if replacement_count != 1:
        raise FormulaUpdateError(
            f"expected exactly one {field_name} field, found {replacement_count}"
        )
    return updated_formula


def update_codex_router_formula(
    *,
    formula_text: str,
    version: str,
    sha256: str,
    source_commit: str,
) -> str:
    """Return the formula updated for one verified codex-router release."""
    if VERSION_PATTERN.fullmatch(version) is None:
        raise FormulaUpdateError("version must use major.minor.patch syntax")
    if SHA256_PATTERN.fullmatch(sha256) is None:
        raise FormulaUpdateError("SHA-256 must be 64 lowercase hexadecimal characters")
    if SOURCE_COMMIT_PATTERN.fullmatch(source_commit) is None:
        raise FormulaUpdateError("source commit must be 40 lowercase hexadecimal characters")

    for required_text, invariant_name in REQUIRED_FORMULA_INVARIANTS:
        if required_text not in formula_text:
            raise FormulaUpdateError(f"formula is missing required {invariant_name}")

    release_url = (
        "https://github.com/ShravanSunder/codex-router/releases/download/"
        f"v{version}/codex-router-v{version}-aarch64-apple-darwin.tar.gz"
    )
    release_url_line = f'  url "{release_url}"'
    updated_formula = _replace_exactly_once(
        formula_text=formula_text,
        pattern=FORMULA_URL_PATTERN,
        replacement=release_url_line,
        field_name="release URL",
    )
    updated_formula = _replace_exactly_once(
        formula_text=updated_formula,
        pattern=FORMULA_SHA256_PATTERN,
        replacement=f'  sha256 "{sha256}"',
        field_name="SHA-256",
    )

    source_commit_line = f"  # Source commit: {source_commit}"
    if FORMULA_SOURCE_COMMIT_PATTERN.search(updated_formula) is not None:
        updated_formula = _replace_exactly_once(
            formula_text=updated_formula,
            pattern=FORMULA_SOURCE_COMMIT_PATTERN,
            replacement=source_commit_line,
            field_name="source commit provenance",
        )
    else:
        if "# Source commit:" in updated_formula:
            raise FormulaUpdateError("formula contains malformed source commit provenance")
        updated_formula = updated_formula.replace(
            release_url_line,
            f"{source_commit_line}\n{release_url_line}",
            1,
        )

    return updated_formula


def update_formula_file(
    *,
    formula_path: Path,
    version: str,
    sha256: str,
    source_commit: str,
) -> bool:
    """Atomically update a formula file and report whether it changed."""
    formula_text = formula_path.read_text(encoding="utf-8")
    updated_formula = update_codex_router_formula(
        formula_text=formula_text,
        version=version,
        sha256=sha256,
        source_commit=source_commit,
    )
    if updated_formula == formula_text:
        return False

    original_mode = stat.S_IMODE(formula_path.stat().st_mode)
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=formula_path.parent,
            prefix=f".{formula_path.name}.",
            delete=False,
        ) as temporary_file:
            temporary_file.write(updated_formula)
            temporary_path = Path(temporary_file.name)
        os.chmod(temporary_path, original_mode)
        os.replace(temporary_path, formula_path)
    finally:
        if temporary_path is not None and temporary_path.exists():
            temporary_path.unlink()

    return True


def parse_arguments() -> argparse.Namespace:
    """Parse formula update arguments."""
    parser = argparse.ArgumentParser(
        description="Update the codex-router Homebrew formula for one verified release."
    )
    parser.add_argument("--formula", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--source-commit", required=True)
    return parser.parse_args()


def main() -> int:
    """Update the requested formula without exposing release credentials."""
    arguments = parse_arguments()
    try:
        changed = update_formula_file(
            formula_path=arguments.formula,
            version=arguments.version,
            sha256=arguments.sha256,
            source_commit=arguments.source_commit,
        )
    except FormulaUpdateError as error:
        print(f"formula update refused: {error}", file=sys.stderr)
        return 1

    print("formula updated" if changed else "formula already current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
