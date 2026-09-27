//! Resolve the owner of private provider-facing sockets once at Host startup.

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

pub(crate) async fn resolve_face_owner_human_id(override_id: Option<HumanId>) -> Option<HumanId> {
    resolve_face_owner_with_command(
        override_id,
        Path::new(OWNER_LOOKUP_COMMAND),
        OWNER_LOOKUP_TIMEOUT,
    )
    .await
}

async fn resolve_face_owner_with_command(
    override_id: Option<HumanId>,
    command_path: &Path,
    timeout: Duration,
) -> Option<HumanId> {
    match resolve_with_command(override_id, command_path, timeout).await {
        Ok(owner) => Some(owner),
        Err(error) => {
            tracing::error!(error_kind = ?error,
                "provider faces unavailable: owner identity lookup failed");
            None
        }
    }
}

async fn resolve_with_command(
    override_id: Option<HumanId>,
    command_path: &Path,
    timeout: Duration,
) -> Result<HumanId, OwnerIdentityError> {
    if let Some(override_id) = override_id {
        return Ok(override_id);
    }
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
    async fn failed_owner_lookup_disables_only_provider_faces() {
        let directory = tempfile::tempdir().expect("test directory");
        let missing = directory.path().join("missing-id");
        assert!(
            resolve_face_owner_with_command(None, &missing, Duration::from_secs(1))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    #[allow(clippy::panic_in_result_fn)]
    async fn owner_lookup_uses_command_result_and_override_skips_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        let script = tempfile::tempdir()?;
        let command_path = script.path().join("id");
        std::fs::write(&command_path, "#!/bin/sh\nprintf 'owner-name\\n'\n")?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&command_path, std::fs::Permissions::from_mode(0o700))?;
        let resolved = resolve_with_command(None, &command_path, Duration::from_secs(5)).await?;
        assert_eq!(resolved.as_str(), "owner-name");

        let override_id = HumanId::try_from("chosen-owner".to_owned())?;
        let resolved = resolve_with_command(
            Some(override_id.clone()),
            Path::new("/nonexistent/id"),
            Duration::from_millis(1),
        )
        .await?;
        assert_eq!(resolved, override_id);
        Ok(())
    }

    #[tokio::test]
    async fn failed_lookup_does_not_create_an_owner() {
        let result =
            resolve_with_command(None, Path::new("/usr/bin/false"), Duration::from_secs(1)).await;
        assert!(matches!(result, Err(OwnerIdentityError::Exit)));
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
            resolve_with_command(None, &command_path, Duration::from_secs(5)).await,
            Err(OwnerIdentityError::Invalid)
        ));
        std::fs::write(&command_path, "#!/bin/sh\nsleep 1\n")?;
        assert!(matches!(
            resolve_with_command(None, &command_path, Duration::from_millis(10)).await,
            Err(OwnerIdentityError::Timeout)
        ));
        Ok(())
    }
}
