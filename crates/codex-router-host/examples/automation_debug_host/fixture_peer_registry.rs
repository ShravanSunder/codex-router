//! Keep the optional Claude peer fixture inside the owned debug run directory.
use super::RunDirectoryAdmission;
use std::{
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

const FIXTURE_DIRECTORY: &str = "claude-peer-fixture";

pub(super) fn prepare(
    run_directory: &Path,
    admission: RunDirectoryAdmission,
    enabled: bool,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let fixture = run_directory.join(FIXTURE_DIRECTORY);
    match (admission, enabled) {
        (RunDirectoryAdmission::Fresh, true) => {
            fs::DirBuilder::new().mode(0o700).create(&fixture)?;
            Ok(Some(fixture))
        }
        (RunDirectoryAdmission::Resume, true) => {
            super::validate_private_owned_directory(&fixture, "Claude peer fixture")?;
            Ok(Some(fixture))
        }
        (RunDirectoryAdmission::Resume, false) if fixture.exists() => Err(io::Error::other(
            "Resume requires --fixture-peer-registry for this owned fixture root",
        )
        .into()),
        (_, false) => Ok(None),
    }
}
