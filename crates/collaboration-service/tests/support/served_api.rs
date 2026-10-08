//! The collaboration API served from a private service directory, the way the Host serves it,
//! for tests that reach the service through the real client or one tool call.
use collaboration_mcp::test_support::ServedCollaborationApi;
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use std::{io, os::unix::fs::PermissionsExt};

pub(crate) struct ServedApi {
    api: ServedCollaborationApi,
    _directory: tempfile::TempDir,
}

impl ServedApi {
    /// Serves `identity`'s application on `control.sock` in a new owner-only directory.
    pub(crate) async fn start(identity: ServiceIdentity) -> io::Result<Self> {
        let directory = tempfile::Builder::new().prefix("api-").tempdir()?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let api = ServedCollaborationApi::start(
            directory.path(),
            CollaborationApplication::new(identity),
        )
        .await?;
        Ok(Self {
            api,
            _directory: directory,
        })
    }

    pub(crate) async fn stop(self) -> io::Result<()> {
        self.api.stop().await
    }
}

impl std::ops::Deref for ServedApi {
    type Target = ServedCollaborationApi;

    fn deref(&self) -> &ServedCollaborationApi {
        &self.api
    }
}
