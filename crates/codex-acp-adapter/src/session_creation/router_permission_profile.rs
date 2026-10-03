//! Per-session half of Router's permission profiles and the check of what native applied.
//!
//! Router launches its managed app-server with the network half of both profiles
//! (`router_profile_projection.rs` in codex-native-integration). Each start, fork and
//! resume adds the parent profile and filesystem grants here, with dotted keys so
//! they merge into that network table instead of replacing it. The tool locations
//! themselves live in `router_tool_locations.rs`, shared with the Host that creates them.
use super::SessionSetupError;
use codex_native_integration::{
    READ_ONLY_INSIDE_TOOL_LOCATIONS, RESTRICTED_TOOL_LOCATIONS, WORKSPACE_TOOL_LOCATIONS,
};
use collaboration_protocol::RouterAccess;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
    read_only_exceptions: BTreeSet<String>,
}

/// Host locations a profile's grants are resolved against.
struct HostLocations {
    home: PathBuf,
    /// Canonical system temporary directories (`$TMPDIR`, `/tmp`), resolved only for
    /// `write-restricted`, whose `:read-only` parent grants none.
    temporary_directories: Vec<PathBuf>,
    /// Tool locations with a symlinked component below `home`. Codex refuses a whole
    /// sandbox that grants a symlinked writable root, so these are not granted.
    symlinked_locations: BTreeSet<&'static str>,
}

impl HostLocations {
    /// Reads the host's `HOME` and, for `write-restricted`, its temporary directories,
    /// as the managed app-server inherits both from this process. Writes nothing: the
    /// Host creates missing tool locations at startup.
    async fn resolve(access: RouterAccess) -> Result<Self, SessionSetupError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or_else(|| SessionSetupError::HostLocationsUnavailable {
                location: "HOME".to_owned(),
            })?;
        let mut temporary_directories = Vec::new();
        if access == RouterAccess::WriteRestricted {
            let candidates = std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .into_iter()
                .chain([PathBuf::from("/tmp")])
                .filter(|path| path.is_absolute());
            for candidate in candidates {
                // Seatbelt matches canonical paths, so `/tmp` is granted as `/private/tmp`.
                // An unresolvable directory fails setup rather than silently losing a grant.
                let canonical = tokio::fs::canonicalize(&candidate).await.map_err(|_| {
                    SessionSetupError::HostLocationsUnavailable {
                        location: candidate.to_string_lossy().into_owned(),
                    }
                })?;
                temporary_directories.push(canonical);
            }
        }
        let mut symlinked_locations = BTreeSet::new();
        for location in tool_locations(access) {
            if has_symlinked_component(&home, location).await {
                symlinked_locations.insert(*location);
            }
        }
        Ok(Self {
            home,
            temporary_directories,
            symlinked_locations,
        })
    }
}

/// The tool locations an access mode may write.
fn tool_locations(access: RouterAccess) -> &'static [&'static str] {
    match access {
        RouterAccess::WorkspaceWrite => &WORKSPACE_TOOL_LOCATIONS,
        RouterAccess::WriteRestricted => &RESTRICTED_TOOL_LOCATIONS,
    }
}

/// Whether any existing component of `location` below `home` is a symlink.
async fn has_symlinked_component(home: &Path, location: &str) -> bool {
    let mut path = home.to_path_buf();
    for component in Path::new(location).components() {
        path.push(component);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => return true,
            Ok(_) => {}
            // A missing component cannot be a symlink, nor can anything below it.
            Err(_) => return false,
        }
    }
    false
}

impl RouterSessionProfile {
    /// Resolves the profile for a session against the host's home and temporary directories.
    pub(super) async fn for_session(
        access: RouterAccess,
        cwd: &Path,
        scratch: &Path,
    ) -> Result<Self, SessionSetupError> {
        let host = HostLocations::resolve(access).await?;
        Ok(Self::with_host(access, cwd, scratch, &host))
    }

    fn with_host(access: RouterAccess, cwd: &Path, scratch: &Path, host: &HostLocations) -> Self {
        let mut writable_roots = BTreeSet::from([scratch.to_string_lossy().into_owned()]);
        writable_roots.extend(
            tool_locations(access)
                .iter()
                .filter(|location| !host.symlinked_locations.contains(*location))
                .map(|location| host.home.join(location).to_string_lossy().into_owned()),
        );
        if access == RouterAccess::WriteRestricted {
            writable_roots.insert(cwd.join("tmp").to_string_lossy().into_owned());
            writable_roots.insert(cwd.join("docs/wip").to_string_lossy().into_owned());
            // `:read-only` grants no temporary directory, yet git, pnpm and SwiftPM
            // write there. Explicit paths keep native's report exact, where the
            // `:tmpdir` token would come back as roots Router cannot predict.
            writable_roots.extend(
                host.temporary_directories
                    .iter()
                    .map(|directory| directory.to_string_lossy().into_owned()),
            );
        }
        let read_only_exceptions = READ_ONLY_INSIDE_TOOL_LOCATIONS
            .iter()
            .map(|location| host.home.join(location).to_string_lossy().into_owned())
            .collect();
        Self {
            access,
            writable_roots,
            read_only_exceptions,
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
            .chain(
                self.read_only_exceptions
                    .iter()
                    .map(|path| (path.clone(), json!("read"))),
            )
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
        // proxy would not be visible here, so the root projection guards it.
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

    fn host(symlinked_locations: BTreeSet<&'static str>) -> HostLocations {
        HostLocations {
            home: PathBuf::from(HOME),
            temporary_directories: TEMPORARY_DIRECTORIES.map(PathBuf::from).to_vec(),
            symlinked_locations,
        }
    }

    fn profile(access: RouterAccess) -> RouterSessionProfile {
        RouterSessionProfile::with_host(
            access,
            Path::new("/repo"),
            Path::new(SCRATCH),
            &host(BTreeSet::new()),
        )
    }

    fn under_home(locations: &[&str]) -> Vec<String> {
        locations
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

    fn filesystem(config: &Map<String, Value>, name: &str) -> Map<String, Value> {
        config[&format!("permissions.{name}.filesystem")]
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn each_mode_grants_scratch_and_its_own_tool_locations() {
        // Arrange
        let workspace = profile(RouterAccess::WorkspaceWrite).native_config();
        let restricted = profile(RouterAccess::WriteRestricted).native_config();

        // Act
        let workspace_filesystem = filesystem(&workspace, "router-workspace-write");
        let restricted_filesystem = filesystem(&restricted, "router-write-restricted");

        // Assert: workspace-write writes toolchain homes; write-restricted only caches and stores.
        for root in under_home(&WORKSPACE_TOOL_LOCATIONS) {
            assert_eq!(workspace_filesystem[&root], "write", "{root}");
        }
        for root in under_home(&RESTRICTED_TOOL_LOCATIONS) {
            assert_eq!(restricted_filesystem[&root], "write", "{root}");
        }
        for toolchain in under_home(&[".rustup", ".cargo", "Library/pnpm"]) {
            assert!(
                !restricted_filesystem.contains_key(&toolchain),
                "{toolchain}"
            );
        }
        for (config, name, parent) in [
            (&workspace, "router-workspace-write", ":workspace"),
            (&restricted, "router-write-restricted", ":read-only"),
        ] {
            assert_eq!(config["default_permissions"], name);
            assert_eq!(config[&format!("permissions.{name}.extends")], parent);
            assert_eq!(filesystem(config, name)[SCRATCH], "write");
            // Assert: no proxy switch; both modes keep the root's direct network.
            assert!(!config.contains_key("features.network_proxy.enabled"));
        }
        // Assert: only write-restricted adds the repository's tmp and docs/wip and the
        // system temporary directories that `:workspace` already grants natively.
        for root in ["/repo/tmp", "/repo/docs/wip"]
            .into_iter()
            .chain(TEMPORARY_DIRECTORIES)
        {
            assert_eq!(restricted_filesystem[root], "write");
            assert!(!workspace_filesystem.contains_key(root));
        }
    }

    #[test]
    fn executables_and_startup_files_stay_read_only_and_are_not_expected_back() {
        // Arrange
        let workspace = profile(RouterAccess::WorkspaceWrite);

        // Act
        let filesystem = filesystem(&workspace.native_config(), "router-workspace-write");

        // Assert: PATH entries and sourced files are read-only inside writable parents.
        for path in [
            ".cargo/bin",
            ".cargo/env",
            "Library/pnpm/bin",
            ".gradle/init.d",
        ] {
            assert_eq!(filesystem[&format!("{HOME}/{path}")], "read", "{path}");
        }
        assert_eq!(filesystem[&format!("{HOME}/.cargo")], "write");
        assert!(
            !workspace
                .writable_roots
                .contains(&format!("{HOME}/.cargo/bin"))
        );
    }

    #[test]
    fn symlinked_tool_locations_are_not_granted() {
        // Arrange: a cache moved to another volume through a symlink.
        let profile = RouterSessionProfile::with_host(
            RouterAccess::WorkspaceWrite,
            Path::new("/repo"),
            Path::new(SCRATCH),
            &host(BTreeSet::from([".cache"])),
        );

        // Act
        let filesystem = filesystem(&profile.native_config(), "router-workspace-write");

        // Assert: Codex refuses a whole sandbox with a symlinked writable root.
        assert!(!filesystem.contains_key(&format!("{HOME}/.cache")));
        assert_eq!(filesystem[&format!("{HOME}/Library/Caches")], "write");
    }

    #[tokio::test]
    async fn symlinked_components_below_home_are_detected() {
        // Arrange
        let home = std::env::temp_dir().join(format!(
            "router-symlink-home-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let elsewhere = home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join(".cache")).unwrap();

        // Act & assert
        assert!(has_symlinked_component(&home, ".cache/uv").await);
        assert!(!has_symlinked_component(&home, "elsewhere").await);
        assert!(!has_symlinked_component(&home, ".missing/child").await);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_settings_must_match_the_requested_profile_with_network() {
        // Arrange
        let workspace = profile(RouterAccess::WorkspaceWrite);
        let mut granted = under_home(&WORKSPACE_TOOL_LOCATIONS);
        granted.push(SCRATCH.to_owned());
        let mut with_unexpected_root = granted.clone();
        with_unexpected_root.push("/other/write-root".to_owned());
        let mut unsandboxed = native_response("router-workspace-write", &granted, true);
        unsandboxed["sandbox"]["type"] = json!("dangerFullAccess");

        // Act & assert: exact profile, roots and network access are accepted.
        assert!(
            workspace
                .validate_observed(&native_response("router-workspace-write", &granted, true))
                .is_ok()
        );
        // Assert: missing settings, extra roots, missing network, another sandbox or
        // the wrong profile are refused.
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
        assert!(workspace.validate_observed(&unsandboxed).is_err());
        assert!(
            workspace
                .validate_observed(&native_response("router-write-restricted", &granted, true))
                .is_err()
        );
    }

    #[test]
    fn write_restricted_expects_its_stores_work_areas_and_temporary_directories() {
        // Arrange
        let restricted = profile(RouterAccess::WriteRestricted);
        let mut granted = under_home(&RESTRICTED_TOOL_LOCATIONS);
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

        // Act & assert
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
