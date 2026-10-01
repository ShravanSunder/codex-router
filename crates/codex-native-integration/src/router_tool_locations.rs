//! Per-user tool locations Router sessions may write, shared by the Host that creates
//! them and the session profile that grants them.
//!
//! Locations are relative to the host's home. A grant covers only its own subtree, so a
//! sandboxed first install cannot create a missing parent; the Host creates missing
//! locations at startup instead.
use std::path::Path;

/// Locations `workspace-write` sessions may write: caches, content stores and the
/// toolchain homes package managers and build systems install into. These are shared
/// tool state: a write here can affect later runs in other sessions and the owner's
/// own builds, which matches what `workspace-write` already allows through source.
pub const WORKSPACE_TOOL_LOCATIONS: [&str; 21] = [
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
    // Python and polyglot tool installs; uv keeps its macOS data here when it exists.
    ".local/share/uv",
    "Library/Application Support/uv",
    ".local/share/mise",
    ".local/state/mise",
];

/// Locations `write-restricted` sessions may write: caches and content stores only.
///
/// Toolchain homes stay read-only because the owner's unsandboxed commands execute
/// from them (`~/.cargo/bin/cargo` dispatches into `~/.rustup/toolchains`, pnpm's `npm`
/// into `~/Library/pnpm/global`), and these agents read untrusted content. Cached
/// sources and build products remain writable; poisoning them is a residual risk.
pub const RESTRICTED_TOOL_LOCATIONS: [&str; 19] = [
    "Library/Caches",
    ".cache/uv",
    "Library/pnpm/store",
    ".local/share/pnpm/store",
    ".pnpm-store",
    ".npm/_cacache",
    ".npm/_logs",
    ".yarn/berry/cache",
    ".bun/install/cache",
    ".cargo/registry",
    ".cargo/git",
    ".cargo/.package-cache",
    ".cargo/.package-cache-mutate",
    ".cargo/.global-cache",
    "go/pkg/mod",
    ".gradle/caches",
    ".m2/repository",
    "Library/org.swift.swiftpm/security",
    "Library/Developer/Xcode/DerivedData",
];

/// Granted entries that are files Cargo creates itself, never directories.
const TOOL_LOCATION_FILES: [&str; 3] = [
    ".cargo/.package-cache",
    ".cargo/.package-cache-mutate",
    ".cargo/.global-cache",
];

/// Paths inside the tool locations that stay read-only: executables on the owner's
/// `PATH` and files the owner's shells source or tools load as configuration or
/// credentials. A session writing these could run code in the owner's next
/// unsandboxed shell or build. Native does not report these exceptions back.
pub const READ_ONLY_INSIDE_TOOL_LOCATIONS: [&str; 18] = [
    "Library/pnpm/bin",
    ".cargo/bin",
    ".cargo/env",
    ".cargo/env.fish",
    ".cargo/env.nu",
    ".cargo/env.ps1",
    ".cargo/env.tcsh",
    ".cargo/config",
    ".cargo/config.toml",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".gradle/init.d",
    ".gradle/init.gradle",
    ".gradle/init.gradle.kts",
    ".gradle/gradle.properties",
    ".m2/settings.xml",
    ".m2/settings-security.xml",
    "Library/Application Support/uv/credentials",
];

/// Creates missing tool-location directories under `home`, best effort.
///
/// Runs in the unsandboxed Host so a session's first install can populate a location
/// whose parent did not exist. A location that cannot be created is logged and left
/// to its tool; it must not stop the Host.
pub async fn prepare_router_tool_locations(home: &Path) {
    let directories = WORKSPACE_TOOL_LOCATIONS
        .iter()
        .chain(RESTRICTED_TOOL_LOCATIONS.iter())
        .filter(|location| !TOOL_LOCATION_FILES.contains(location));
    for location in directories {
        let path = home.join(location);
        if let Err(error) = tokio::fs::create_dir_all(&path).await {
            tracing::warn!(
                location = %path.display(),
                error = %error,
                "could not create Router tool location"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_directories_and_parents_are_created_but_cargo_files_are_not() {
        // Arrange: an empty home, so every location and parent (`.bun`, `go`, `.local`) is absent.
        let home = std::env::temp_dir().join(format!(
            "router-tool-locations-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&home).unwrap();

        // Act
        prepare_router_tool_locations(&home).await;

        // Assert
        for location in WORKSPACE_TOOL_LOCATIONS
            .iter()
            .chain(RESTRICTED_TOOL_LOCATIONS.iter())
            .filter(|location| !TOOL_LOCATION_FILES.contains(location))
        {
            assert!(home.join(location).is_dir(), "{location} was not created");
        }
        for file in TOOL_LOCATION_FILES {
            assert!(
                !home.join(file).exists(),
                "{file} must stay for Cargo to create"
            );
        }
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn restricted_locations_exclude_toolchain_homes_and_executable_caches() {
        // Assert: nothing an unsandboxed command executes from is writable to write-restricted.
        for executed in [
            ".rustup",
            ".cargo",
            ".cargo/bin",
            "Library/pnpm",
            "Library/pnpm/global",
            ".npm",
            ".npm/_npx",
            ".cache",
            ".cache/pre-commit",
            ".local/share/mise",
            ".gradle",
            ".m2",
        ] {
            assert!(
                !RESTRICTED_TOOL_LOCATIONS.contains(&executed),
                "{executed} must stay read-only for write-restricted"
            );
        }
    }
}
