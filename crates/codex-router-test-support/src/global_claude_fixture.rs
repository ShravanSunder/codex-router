//! Scoped native executable setup for tests of common Claude provider startup.

use std::{fs, io, path::Path};

/// Declares a test-owned executable without depending on the machine's Claude install.
pub fn declare_global_claude_fixture(
    root: &Path,
    environment: &mut Vec<(String, String)>,
) -> io::Result<()> {
    let directory = root.join("global-claude-fixture");
    fs::create_dir_all(&directory)?;
    let executable = directory.join("claude");
    fs::write(&executable, "#!/bin/sh\nprintf 'fixture-claude\\n'\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    }
    let inherited = std::env::var_os("PATH");
    let existing = environment
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| std::ffi::OsStr::new(value))
        .or(inherited.as_deref());
    let mut entries = vec![directory];
    if let Some(existing) = existing {
        entries.extend(std::env::split_paths(existing));
    }
    let search_path = std::env::join_paths(entries).map_err(io::Error::other)?;
    let search_path = search_path
        .into_string()
        .map_err(|_| io::Error::other("fixture PATH must be UTF-8"))?;
    environment.retain(|(name, _)| name != "PATH");
    environment.push(("PATH".to_owned(), search_path));
    Ok(())
}
