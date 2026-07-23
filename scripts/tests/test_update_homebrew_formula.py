import unittest

from scripts.update_homebrew_formula import FormulaUpdateError
from scripts.update_homebrew_formula import update_codex_router_formula


BASE_FORMULA = '''class CodexRouter < Formula
  desc "Local account and quota router for Codex CLI"
  homepage "https://github.com/ShravanSunder/codex-router"
  url "https://github.com/ShravanSunder/codex-router/releases/download/v0.1.2/codex-router-v0.1.2-aarch64-apple-darwin.tar.gz"
  sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  license any_of: ["MIT", "Apache-2.0"]

  depends_on arch: :arm64
  depends_on :macos

  def install
    bin.install "codex-router"
  end
end
'''


class UpdateCodexRouterFormulaTests(unittest.TestCase):
    def test_updates_release_fields_and_inserts_source_provenance(self) -> None:
        # Arrange
        expected_sha256 = "b" * 64
        expected_source_commit = "c" * 40

        # Act
        updated_formula = update_codex_router_formula(
            formula_text=BASE_FORMULA,
            version="0.2.0",
            sha256=expected_sha256,
            source_commit=expected_source_commit,
        )

        # Assert
        self.assertIn(
            "releases/download/v0.2.0/codex-router-v0.2.0-aarch64-apple-darwin.tar.gz",
            updated_formula,
        )
        self.assertIn(f'  sha256 "{expected_sha256}"', updated_formula)
        self.assertIn(f"  # Source commit: {expected_source_commit}", updated_formula)

    def test_second_update_is_idempotent(self) -> None:
        # Arrange
        sha256 = "d" * 64
        source_commit = "e" * 40
        first_update = update_codex_router_formula(
            formula_text=BASE_FORMULA,
            version="0.2.0",
            sha256=sha256,
            source_commit=source_commit,
        )

        # Act
        second_update = update_codex_router_formula(
            formula_text=first_update,
            version="0.2.0",
            sha256=sha256,
            source_commit=source_commit,
        )

        # Assert
        self.assertEqual(second_update, first_update)
        self.assertEqual(second_update.count("# Source commit:"), 1)

    def test_rejects_malformed_sha256(self) -> None:
        # Arrange
        malformed_sha256 = "not-a-sha256"

        # Act / Assert
        with self.assertRaisesRegex(FormulaUpdateError, "SHA-256"):
            update_codex_router_formula(
                formula_text=BASE_FORMULA,
                version="0.2.0",
                sha256=malformed_sha256,
                source_commit="f" * 40,
            )

    def test_rejects_malformed_version(self) -> None:
        # Arrange
        malformed_version = "v0.2"

        # Act / Assert
        with self.assertRaisesRegex(FormulaUpdateError, "version"):
            update_codex_router_formula(
                formula_text=BASE_FORMULA,
                version=malformed_version,
                sha256="a" * 64,
                source_commit="f" * 40,
            )

    def test_rejects_formula_without_apple_silicon_requirement(self) -> None:
        # Arrange
        formula_without_arm64_requirement = BASE_FORMULA.replace(
            "  depends_on arch: :arm64\n",
            "",
        )

        # Act / Assert
        with self.assertRaisesRegex(FormulaUpdateError, "Apple Silicon"):
            update_codex_router_formula(
                formula_text=formula_without_arm64_requirement,
                version="0.2.0",
                sha256="a" * 64,
                source_commit="f" * 40,
            )


if __name__ == "__main__":
    unittest.main()
