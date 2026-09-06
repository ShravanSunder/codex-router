//! Join existing child lifecycle facts to the independent public communication runtime.
use crate::{
    AppServerChild, BackendSchemaEvidence, CommunicationRuntime, CommunicationRuntimeInputs,
    HostConfig,
};
use std::{io, path::PathBuf};

pub(super) struct CommunicationLifecycle {
    directory: PathBuf,
    codex_home: PathBuf,
    backend_socket: PathBuf,
    runtime: Option<CommunicationRuntime>,
    published_child: Option<u32>,
    schema_digest: Option<[u8; 32]>,
}
impl CommunicationLifecycle {
    pub(super) async fn start(
        config: &HostConfig,
        child: &AppServerChild,
    ) -> io::Result<Option<Self>> {
        let Some(directory) = config.communication_directory() else {
            return Ok(None);
        };
        let mut owner = Self {
            directory: directory.to_owned(),
            codex_home: config
                .communication_codex_home()
                .ok_or_else(|| io::Error::other("communication requires explicit Codex home"))?
                .to_owned(),
            backend_socket: config.app_server_socket().to_owned(),
            runtime: None,
            published_child: None,
            schema_digest: None,
        };
        owner.synchronize(Some(child)).await?;
        Ok(Some(owner))
    }
    pub(super) async fn synchronize(&mut self, child: Option<&AppServerChild>) -> io::Result<()> {
        let Some(child) = child else {
            if self.published_child.take().is_some()
                && let Some(runtime) = &mut self.runtime
            {
                runtime
                    .backend_unavailable(
                        timestamp()?,
                        "Native backend transitioning"
                            .to_owned()
                            .try_into()
                            .map_err(io::Error::other)?,
                    )
                    .await?;
            }
            return Ok(());
        };
        if self.published_child == Some(child.process.process_id()) {
            return Ok(());
        }
        let export = child.schema_export();
        let digest = export.as_ref().map(|export| *export.bundle().digest());
        if self.runtime.is_some() && digest != self.schema_digest {
            // A changed Control payload profile cannot flow over old initialized connections.
            self.shutdown().await?;
        }
        if self.runtime.is_none() {
            use std::os::unix::fs::DirBuilderExt;
            match std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&self.directory)
            {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            self.runtime = Some(
                CommunicationRuntime::start(CommunicationRuntimeInputs {
                    directory: self.directory.clone(),
                    codex_home: self.codex_home.clone(),
                    backend_socket: self.backend_socket.clone(),
                    native_schema: export.clone(),
                })
                .await?,
            );
            self.schema_digest = digest;
        }
        let runtime = self
            .runtime
            .as_mut()
            .ok_or_else(|| io::Error::other("communication runtime missing"))?;
        runtime
            .backend_ready(
                timestamp()?,
                export.as_ref().map(|export| BackendSchemaEvidence {
                    running_executable: child.identity(),
                    export,
                }),
            )
            .await?;
        self.published_child = Some(child.process.process_id());
        Ok(())
    }
    pub(super) async fn listener_failure(&mut self) -> io::Error {
        match &mut self.runtime {
            Some(runtime) => runtime.listener_failure().await,
            None => std::future::pending().await,
        }
    }
    pub(super) async fn shutdown(&mut self) -> io::Result<()> {
        self.published_child = None;
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await?;
        }
        Ok(())
    }
}
pub(super) async fn wait_failure(owner: &mut Option<CommunicationLifecycle>) -> io::Error {
    match owner {
        Some(owner) => owner.listener_failure().await,
        None => std::future::pending().await,
    }
}
fn timestamp() -> io::Result<communication_protocol::ObservationTimestamp> {
    chrono::Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        .try_into()
        .map_err(io::Error::other)
}
