import unittest

from scripts.publish_homebrew_tap_local import PublishError
from scripts.publish_homebrew_tap_local import install_action
from scripts.publish_homebrew_tap_local import published_asset_sha256
from scripts.publish_homebrew_tap_local import release_signature_problem
from scripts.publish_homebrew_tap_local import release_asset_name
from scripts.publish_homebrew_tap_local import workspace_version


CARGO_TOML = """[workspace]
resolver = "3"
members = ["crates/a"]

[workspace.package]
edition = "2024"
version = "0.1.56"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
version_check = { version = "9.9.9" }
"""


class WorkspaceVersionTests(unittest.TestCase):
    def test_reads_the_workspace_package_version_not_a_dependency_version(self) -> None:
        # Act
        version = workspace_version(CARGO_TOML)

        # Assert
        self.assertEqual(version, "0.1.56")

    def test_refuses_a_manifest_without_a_workspace_version(self) -> None:
        # Act & Assert
        with self.assertRaises(PublishError):
            _ = workspace_version('[package]\nversion = "1.0.0"\n')


class ReleaseAssetTests(unittest.TestCase):
    def test_names_the_apple_silicon_asset_the_formula_installs(self) -> None:
        # Act & Assert
        self.assertEqual(
            release_asset_name("0.1.56"),
            "codex-router-v0.1.56-aarch64-apple-darwin.tar.gz",
        )

    def test_returns_the_recorded_sha256_of_the_named_asset(self) -> None:
        # Arrange
        release: dict[str, object] = {
            "assets": [
                {"name": "other.tar.gz", "digest": "sha256:" + "a" * 64},
                {"name": release_asset_name("0.1.56"), "digest": "sha256:" + "b" * 64},
            ]
        }

        # Act
        sha256 = published_asset_sha256(release, release_asset_name("0.1.56"))

        # Assert
        self.assertEqual(sha256, "b" * 64)

    def test_refuses_a_missing_asset_or_a_malformed_digest(self) -> None:
        # Arrange
        name = release_asset_name("0.1.56")
        missing: dict[str, object] = {"assets": [{"name": "other.tar.gz", "digest": None}]}
        malformed: dict[str, object] = {"assets": [{"name": name, "digest": "md5:abc"}]}
        no_assets: dict[str, object] = {}

        # Act & Assert
        for release in (missing, malformed, no_assets):
            with self.assertRaises(PublishError):
                _ = published_asset_sha256(release, name)


class InstallActionTests(unittest.TestCase):
    def test_installs_fresh_when_absent_and_upgrades_from_an_older_version(self) -> None:
        # Act & Assert
        self.assertEqual(install_action("", "0.1.56"), "install")
        self.assertEqual(install_action("codex-router 0.1.55", "0.1.56"), "upgrade")

    def test_reinstalls_a_same_version_so_the_install_proof_still_runs(self) -> None:
        # Act & Assert
        self.assertEqual(install_action("codex-router 0.1.56", "0.1.56"), "reinstall")


class ReleaseSignatureTests(unittest.TestCase):
    def test_accepts_the_release_identifier_signed_by_the_release_team(self) -> None:
        # Arrange
        details = "Identifier=dev.shravansunder.agent-sessions\nTeamIdentifier=974QD84WVC\n"

        # Act & Assert
        self.assertIsNone(release_signature_problem(details, "agent-sessions"))

    def test_rejects_linker_signed_and_debug_identities(self) -> None:
        # Arrange
        linker_signed = "Identifier=agent_sessions-349f0fe06f58ee87\nTeamIdentifier=not set\n"
        debug_signed = "Identifier=dev.shravansunder.agent-sessions.debug\nTeamIdentifier=974QD84WVC\n"
        other_team = "Identifier=dev.shravansunder.agent-sessions\nTeamIdentifier=AAAAAAAAAA\n"

        # Act & Assert
        for details in (linker_signed, debug_signed, other_team):
            self.assertIsNotNone(release_signature_problem(details, "agent-sessions"))


if __name__ == "__main__":
    _ = unittest.main()
