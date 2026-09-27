//! Resolve the OS owner identity shared by CLI and Host provider front doors.

use message_board::HumanId;
use std::{path::Path, time::Duration};
use tokio::process::Command;

const OWNER_LOOKUP_TIMEOUT: Duration = Duration::from_secs(1);
const OWNER_LOOKUP_COMMAND: &str = "/usr/bin/id";

#[derive(Debug, thiserror::Error)]
pub enum OwnerIdentityError {
    #[error("owner account lookup could not start")]
    Spawn(#[source] std::io::Error),
    #[error("owner account lookup timed out")]
    Timeout,
    #[error("owner account lookup failed")]
    Exit,
    #[error("owner account lookup returned an invalid HumanId")]
    Invalid,
}

/// Resolves the current OS account to the Human ID used by provider faces.
pub async fn resolve_owner_human_id() -> Result<HumanId, OwnerIdentityError> {
    resolve_with_command(Path::new(OWNER_LOOKUP_COMMAND), OWNER_LOOKUP_TIMEOUT).await
}

async fn resolve_with_command(
    command_path: &Path,
    timeout: Duration,
) -> Result<HumanId, OwnerIdentityError> {
    let mut command = Command::new(command_path);
    command.arg("-un").kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| OwnerIdentityError::Timeout)?
        .map_err(OwnerIdentityError::Spawn)?;
    if !output.status.success() {
        return Err(OwnerIdentityError::Exit);
    }
    let name = std::str::from_utf8(&output.stdout)
        .map_err(|_| OwnerIdentityError::Invalid)?
        .trim();
    if name.contains(['\n', '\r']) {
        return Err(OwnerIdentityError::Invalid);
    }
    HumanId::try_from(name.to_owned()).map_err(|_| OwnerIdentityError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::panic_in_result_fn)]
    async fn owner_lookup_uses_command_result() -> Result<(), Box<dyn std::error::Error>> {
        let script = tempfile::tempdir()?;
        let command_path = script.path().join("id");
        std::fs::write(&command_path, "#!/bin/sh\nprintf 'owner-name\\n'\n")?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&command_path, std::fs::Permissions::from_mode(0o700))?;
        let resolved = resolve_with_command(&command_path, Duration::from_secs(5)).await?;
        assert_eq!(resolved.as_str(), "owner-name");
        Ok(())
    }

    #[tokio::test]
    async fn failed_lookup_does_not_create_an_owner() {
        let directory = tempfile::tempdir().expect("test directory");
        let missing = directory.path().join("missing-id");
        assert!(matches!(
            resolve_with_command(&missing, Duration::from_secs(1)).await,
            Err(OwnerIdentityError::Spawn(_))
        ));
        assert!(matches!(
            resolve_with_command(Path::new("/usr/bin/false"), Duration::from_secs(1)).await,
            Err(OwnerIdentityError::Exit)
        ));
    }

    #[tokio::test]
    #[allow(clippy::panic_in_result_fn)]
    async fn invalid_or_stalled_account_lookup_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let script = tempfile::tempdir()?;
        let command_path = script.path().join("id");
        std::fs::write(&command_path, "#!/bin/sh\nprintf 'first\\nsecond\\n'\n")?;
        std::fs::set_permissions(&command_path, std::fs::Permissions::from_mode(0o700))?;
        assert!(matches!(
            resolve_with_command(&command_path, Duration::from_secs(5)).await,
            Err(OwnerIdentityError::Invalid)
        ));
        std::fs::write(&command_path, "#!/bin/sh\nsleep 1\n")?;
        assert!(matches!(
            resolve_with_command(&command_path, Duration::from_millis(10)).await,
            Err(OwnerIdentityError::Timeout)
        ));
        Ok(())
    }
}
