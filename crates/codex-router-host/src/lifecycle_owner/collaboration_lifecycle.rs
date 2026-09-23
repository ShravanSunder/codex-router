//! Join existing child lifecycle facts to the independent public collaboration runtime.
use crate::{
    AppServerChild, BackendSchemaEvidence, CollaborationRuntime, CollaborationRuntimeInputs,
    HostConfig,
};
use std::{io, path::PathBuf};

pub(super) struct CollaborationLifecycle {
    directory: PathBuf,
    codex_home: PathBuf,
    backend_socket: PathBuf,
    mcp_bind: std::net::SocketAddr,
    external_provider_launches: Vec<crate::ExternalProviderLaunchBinding>,
    provider_operation_retention_days: std::num::NonZeroU32,
    runtime: Option<CollaborationRuntime>,
    published_child: Option<u32>,
    schema_digest: Option<[u8; 32]>,
}
impl CollaborationLifecycle {
    pub(super) async fn start(
        config: &HostConfig,
        child: &AppServerChild,
    ) -> io::Result<Option<Self>> {
        let Some(directory) = config.collaboration_directory() else {
            return Ok(None);
        };
        let mut owner = Self {
            directory: directory.to_owned(),
            codex_home: config
                .collaboration_codex_home()
                .ok_or_else(|| io::Error::other("collaboration requires explicit Codex home"))?
                .to_owned(),
            backend_socket: config.app_server_socket().to_owned(),
            mcp_bind: config.mcp_bind(),
            external_provider_launches: config.external_provider_launches().to_vec(),
            provider_operation_retention_days: config.provider_operation_retention_days(),
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
            let mut runtime = CollaborationRuntime::start_with_external_providers(
                CollaborationRuntimeInputs {
                    directory: self.directory.clone(),
                    codex_home: self.codex_home.clone(),
                    backend_socket: self.backend_socket.clone(),
                    mcp_bind: self.mcp_bind,
                    native_schema: export.clone(),
                },
                self.external_provider_launches.clone(),
            )
            .await?;
            runtime
                .configure_provider_operation_retention(self.provider_operation_retention_days)
                .await?;
            self.runtime = Some(runtime);
            self.schema_digest = digest;
        }
        let runtime = self
            .runtime
            .as_mut()
            .ok_or_else(|| io::Error::other("collaboration runtime missing"))?;
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
pub(super) async fn wait_failure(owner: &mut Option<CollaborationLifecycle>) -> io::Error {
    match owner {
        Some(owner) => owner.listener_failure().await,
        None => std::future::pending().await,
    }
}
fn timestamp() -> io::Result<collaboration_protocol::ObservationTimestamp> {
    chrono::Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        .try_into()
        .map_err(io::Error::other)
}
