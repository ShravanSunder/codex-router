//! Give protocol fixtures their own installed-Claude prerequisite without host PATH state.

use crate::ExternalProviderLaunchBinding;
use std::{fs, io, os::unix::fs::PermissionsExt as _, path::Path};

pub(crate) fn with_global_claude_fixture(
    root: &Path,
    mut binding: ExternalProviderLaunchBinding,
) -> io::Result<ExternalProviderLaunchBinding> {
    let global_directory = root.join("global-bin");
    fs::create_dir_all(&global_directory)?;
    fs::set_permissions(&global_directory, fs::Permissions::from_mode(0o700))?;
    let executable = global_directory.join("claude");
    fs::write(
        &executable,
        "#!/usr/bin/python3\nimport sys\nassert sys.argv[1:] == ['--version']\nprint('host-test-global-claude-v1')\n",
    )?;
    fs::set_permissions(executable, fs::Permissions::from_mode(0o700))?;
    binding
        .launch
        .environment
        .retain(|(name, _)| name != "PATH" && name != "CLAUDE_CODE_EXECUTABLE");
    binding.launch.environment.extend([
        (
            "PATH".to_owned(),
            global_directory.to_string_lossy().into_owned(),
        ),
        ("CLAUDE_CODE_EXECUTABLE".to_owned(), String::new()),
    ]);
    Ok(binding)
}
