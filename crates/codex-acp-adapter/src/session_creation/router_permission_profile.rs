//! Per-session half of Router's permission profiles and the check of what native applied.
//!
//! Router launches its managed app-server with the network half of both profiles
//! (`router_profile_projection.rs` in codex-native-integration). Each start, fork and
//! resume adds the parent profile and filesystem grants here, with dotted keys so
//! they merge into that network table instead of replacing it.
use super::SessionSetupError;
use collaboration_protocol::RouterAccess;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Per-user tool locations every Router session may write, relative to the host's home.
///
/// Package managers, toolchains and build systems keep shared caches, stores and
/// toolchain state here; without them `pnpm install`, Cargo, SwiftPM and Xcode fail
/// outside the working directory. These are shared tool state, not only caches: a
/// write here can affect later runs in other sessions. Locations that do not exist yet
/// are granted too, so a first install can create them.
const TOOL_LOCATIONS_UNDER_HOME: [&str; 21] = [
    // Platform and XDG caches: SwiftPM, Xcode, clang modules, Go builds, Homebrew, pip, uv.
    "Library/Caches",
    ".cache",
    // JavaScript package managers, their stores and self-managed versions.
    "Library/pnpm",
    ".local/share/pnpm",
    ".local/state/pnpm",
    ".pnpm-store",
    ".npm",
    ".yarn",
    ".bun/install/cache",
    ".local/state/fnm_multishells",
    // Rust toolchains and Cargo's registry, git checkouts and package-cache lock.
    ".cargo",
    ".rustup",
    // Go modules and JVM dependency caches.
    "go/pkg",
    ".gradle",
    ".m2",
    // Swift package state and Xcode derived data and simulators.
    "Library/org.swift.swiftpm",
    ".swiftpm",
    "Library/Developer",
    // Python and polyglot tool installs.
    ".local/share/uv",
    ".local/share/mise",
    ".local/state/mise",
];

const fn profile_name(access: RouterAccess) -> &'static str {
    match access {
        RouterAccess::WriteRestricted => "router-write-restricted",
        RouterAccess::WorkspaceWrite => "router-workspace-write",
    }
}

pub(super) const fn access_name(access: RouterAccess) -> &'static str {
    match access {
        RouterAccess::WriteRestricted => "write-restricted",
        RouterAccess::WorkspaceWrite => "workspace-write",
    }
}

const fn parent_profile(access: RouterAccess) -> &'static str {
    match access {
        RouterAccess::WriteRestricted => ":read-only",
        RouterAccess::WorkspaceWrite => ":workspace",
    }
}

/// The profile one Router session selects: its access and every root it may write
/// beyond what the parent profile already grants.
pub(super) struct RouterSessionProfile {
    access: RouterAccess,
    writable_roots: BTreeSet<String>,
}

/// Host locations a profile's grants are resolved against.
struct HostLocations {
    home: PathBuf,
    /// Canonical system temporary directories: `$TMPDIR` and `/tmp`.
    temporary_directories: Vec<PathBuf>,
}

impl HostLocations {
    /// Reads the host's `HOME` and resolves its temporary directories, as the
    /// managed app-server inherits both from this process.
    async fn resolve() -> Result<Self, SessionSetupError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or(SessionSetupError::HomeDirectoryUnavailable)?;
        let mut temporary_directories = Vec::new();
        let candidates = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .into_iter()
            .chain([PathBuf::from("/tmp")])
            .filter(|path| path.is_absolute());
        for candidate in candidates {
            // Seatbelt matches canonical paths, so `/tmp` is granted as `/private/tmp`.
            if let Ok(canonical) = tokio::fs::canonicalize(&candidate).await {
                temporary_directories.push(canonical);
            }
        }
        Ok(Self {
            home,
            temporary_directories,
        })
    }
}

impl RouterSessionProfile {
    /// Resolves the profile for a session against the host's home and temporary directories.
    pub(super) async fn for_session(
        access: RouterAccess,
        cwd: &Path,
        scratch: &Path,
    ) -> Result<Self, SessionSetupError> {
        Ok(Self::with_host(
            access,
            cwd,
            scratch,
            &HostLocations::resolve().await?,
        ))
    }

    fn with_host(access: RouterAccess, cwd: &Path, scratch: &Path, host: &HostLocations) -> Self {
        let mut writable_roots = BTreeSet::from([scratch.to_string_lossy().into_owned()]);
        writable_roots.extend(
            TOOL_LOCATIONS_UNDER_HOME
                .iter()
                .map(|location| host.home.join(location).to_string_lossy().into_owned()),
        );
        if access == RouterAccess::WriteRestricted {
            writable_roots.insert(cwd.join("tmp").to_string_lossy().into_owned());
            writable_roots.insert(cwd.join("docs/wip").to_string_lossy().into_owned());
            // `:read-only` grants no temporary directory, yet git signing, pnpm and
            // SwiftPM write there. Explicit paths keep native's report exact, where
            // the `:tmpdir` token would come back as roots Router cannot predict.
            writable_roots.extend(
                host.temporary_directories
                    .iter()
                    .map(|directory| directory.to_string_lossy().into_owned()),
            );
        }
        Self {
            access,
            writable_roots,
        }
    }

    pub(super) const fn name(&self) -> &'static str {
        profile_name(self.access)
    }

    /// Native `config` entries selecting this profile on start, fork or resume.
    pub(super) fn native_config(&self) -> Map<String, Value> {
        let profile = self.name();
        let filesystem = self
            .writable_roots
            .iter()
            .map(|root| (root.clone(), json!("write")))
            .collect::<Map<_, _>>();
        Map::from_iter([
            ("default_permissions".to_owned(), json!(profile)),
            (
                format!("permissions.{profile}.extends"),
                json!(parent_profile(self.access)),
            ),
            (
                format!("permissions.{profile}.filesystem"),
                Value::Object(filesystem),
            ),
        ])
    }

    /// Refuses a native start, fork or resume whose effective settings differ from this profile.
    pub(super) fn validate_observed(&self, response: &Value) -> Result<(), SessionSetupError> {
        let profile_id = response
            .pointer("/activePermissionProfile/id")
            .and_then(Value::as_str)
            .ok_or(SessionSetupError::ConfigurationMismatch)?;
        let actual = response
            .pointer("/sandbox/writableRoots")
            .and_then(Value::as_array)
            .ok_or(SessionSetupError::ConfigurationMismatch)?
            .iter()
            .map(|root| {
                root.as_str()
                    .map(str::to_owned)
                    .ok_or(SessionSetupError::ConfigurationMismatch)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let sandbox_type = response.pointer("/sandbox/type").and_then(Value::as_str);
        let network_access = response
            .pointer("/sandbox/networkAccess")
            .and_then(Value::as_bool);
        // Both profiles surface as a writable-roots sandbox with direct network; a
        // proxied network is not visible here, so the root projection guards it.
        // Temporary-directory flags are not compared: both profiles write them.
        if profile_id != self.name()
            || sandbox_type != Some("workspaceWrite")
            || actual != self.writable_roots
            || network_access != Some(true)
        {
            return Err(SessionSetupError::AccessMismatch {
                requested: access_name(self.access).to_owned(),
                effective: format!(
                    "profile={profile_id}, sandbox={sandbox_type:?}, roots={actual:?}, networkAccess={network_access:?}"
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/owner";
    const SCRATCH: &str = "/owner/scratch/root";
    const TEMPORARY_DIRECTORIES: [&str; 2] = ["/private/var/folders/xy/T", "/private/tmp"];

    fn profile(access: RouterAccess) -> RouterSessionProfile {
        RouterSessionProfile::with_host(
            access,
            Path::new("/repo"),
            Path::new(SCRATCH),
            &HostLocations {
                home: PathBuf::from(HOME),
                temporary_directories: TEMPORARY_DIRECTORIES.map(PathBuf::from).to_vec(),
            },
        )
    }

    fn tool_roots() -> Vec<String> {
        TOOL_LOCATIONS_UNDER_HOME
            .iter()
            .map(|location| format!("{HOME}/{location}"))
            .collect()
    }

    fn native_response(profile_id: &str, roots: &[String], network_access: bool) -> Value {
        json!({
            "activePermissionProfile":{"id":profile_id},
            "sandbox":{
                "type":"workspaceWrite",
                "writableRoots":roots,
                "networkAccess":network_access,
                "excludeTmpdirEnvVar":false,
                "excludeSlashTmp":false
            }
        })
    }

    #[test]
    fn both_profiles_grant_scratch_and_every_tool_location() {
        // Arrange
        let workspace = profile(RouterAccess::WorkspaceWrite);
        let restricted = profile(RouterAccess::WriteRestricted);

        // Act
        let workspace_config = workspace.native_config();
        let restricted_config = restricted.native_config();

        // Assert: pnpm, Cargo and SwiftPM stores are writable in both access modes.
        for (config, name) in [
            (&workspace_config, "router-workspace-write"),
            (&restricted_config, "router-write-restricted"),
        ] {
            let filesystem = config[&format!("permissions.{name}.filesystem")]
                .as_object()
                .unwrap();
            assert_eq!(filesystem[SCRATCH], "write");
            for root in tool_roots() {
                assert_eq!(filesystem[&root], "write", "{name} misses {root}");
            }
            assert_eq!(config["default_permissions"], name);
        }
        assert_eq!(
            workspace_config["permissions.router-workspace-write.extends"],
            ":workspace"
        );
        assert_eq!(
            restricted_config["permissions.router-write-restricted.extends"],
            ":read-only"
        );
        // Assert: only write-restricted adds the repository's tmp and docs/wip and the
        // system temporary directories that `:workspace` already grants natively.
        let restricted_filesystem =
            restricted_config["permissions.router-write-restricted.filesystem"]
                .as_object()
                .unwrap();
        let workspace_filesystem =
            workspace_config["permissions.router-workspace-write.filesystem"]
                .as_object()
                .unwrap();
        for root in ["/repo/tmp", "/repo/docs/wip"]
            .into_iter()
            .chain(TEMPORARY_DIRECTORIES)
        {
            assert_eq!(restricted_filesystem[root], "write");
            assert!(!workspace_filesystem.contains_key(root));
        }
    }

    #[test]
    fn native_settings_must_match_the_requested_profile_with_network() {
        // Arrange
        let workspace = profile(RouterAccess::WorkspaceWrite);
        let mut granted = tool_roots();
        granted.push(SCRATCH.to_owned());
        let mut with_unexpected_root = granted.clone();
        with_unexpected_root.push("/other/write-root".to_owned());

        // Act & assert: exact profile, roots and network access are accepted.
        assert!(
            workspace
                .validate_observed(&native_response("router-workspace-write", &granted, true))
                .is_ok()
        );
        // Assert: missing settings, extra roots, missing network or the wrong profile are refused.
        assert!(workspace.validate_observed(&json!({})).is_err());
        assert!(
            workspace
                .validate_observed(&native_response(
                    "router-workspace-write",
                    &with_unexpected_root,
                    true
                ))
                .is_err()
        );
        assert!(
            workspace
                .validate_observed(&native_response("router-workspace-write", &granted, false))
                .is_err()
        );
        assert!(
            workspace
                .validate_observed(&native_response("router-write-restricted", &granted, true))
                .is_err()
        );
        let mut unsandboxed = native_response("router-workspace-write", &granted, true);
        unsandboxed["sandbox"]["type"] = json!("dangerFullAccess");
        assert!(workspace.validate_observed(&unsandboxed).is_err());
    }

    #[test]
    fn write_restricted_expects_its_work_areas_and_temporary_directories() {
        // Arrange: native folds explicit temporary directories into its own temp flags.
        let restricted = profile(RouterAccess::WriteRestricted);
        let mut granted = tool_roots();
        granted.extend(
            [SCRATCH, "/repo/tmp", "/repo/docs/wip"]
                .into_iter()
                .chain(TEMPORARY_DIRECTORIES)
                .map(str::to_owned),
        );
        let without_temporary = granted
            .iter()
            .filter(|root| !TEMPORARY_DIRECTORIES.contains(&root.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        // Act & assert: the full grant set is accepted; a report missing temp is refused.
        assert!(
            restricted
                .validate_observed(&native_response("router-write-restricted", &granted, true))
                .is_ok()
        );
        assert!(
            restricted
                .validate_observed(&native_response(
                    "router-write-restricted",
                    &without_temporary,
                    true
                ))
                .is_err()
        );
    }
}
