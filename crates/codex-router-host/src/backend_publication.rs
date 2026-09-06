//! Lifecycle-owned endpoint publication without process-control authority in clients.
use communication_protocol::{
    ChannelDescription, CodexGeneration, EndpointAvailability, EndpointDescription, EndpointRef,
    GenerationNumber, NativeCarrier, NonEmptyText, ObservationTimestamp, SchemaDigest,
    UuidIdentity,
};
use communication_service::{EndpointDirectory, NativeGenerationGate};
use std::io;
use std::path::PathBuf;

pub struct BackendPublication {
    directory: EndpointDirectory,
    endpoint: EndpointRef,
    epoch: UuidIdentity,
    generation: u64,
    label: NonEmptyText,
    native_path: NonEmptyText,
    accepting: bool,
    acp_schema: Option<SchemaDigest>,
    acp_path: NonEmptyText,
    acp_supported: bool,
    admission: NativeGenerationGate,
    backend_path: PathBuf,
}
impl BackendPublication {
    pub fn new(
        directory: EndpointDirectory,
        endpoint: EndpointRef,
        epoch: UuidIdentity,
        backend_path: PathBuf,
    ) -> io::Result<Self> {
        if !backend_path.is_absolute() {
            return Err(io::Error::other("backend path must be absolute"));
        }
        Ok(Self {
            directory,
            endpoint,
            epoch,
            generation: 0,
            label: NonEmptyText::try_from("Local Codex".to_owned()).map_err(io::Error::other)?,
            native_path: NonEmptyText::try_from("codex-native.sock".to_owned())
                .map_err(io::Error::other)?,
            accepting: false,
            acp_schema: None,
            acp_path: NonEmptyText::try_from("codex-acp.sock".to_owned())
                .map_err(io::Error::other)?,
            acp_supported: false,
            admission: NativeGenerationGate::default(),
            backend_path,
        })
    }
    pub fn with_acp_listener(mut self) -> io::Result<Self> {
        self.acp_schema = Some(
            format!("sha256:{}", communication_service::ACP_SCHEMA_DIGEST)
                .try_into()
                .map_err(io::Error::other)?,
        );
        Ok(self)
    }
    #[must_use]
    pub fn admission_gate(&self) -> NativeGenerationGate {
        self.admission.clone()
    }

    /// Publish only after the lifecycle owner observes backend and public-carrier readiness.
    pub fn ready(
        &mut self,
        observed_at: ObservationTimestamp,
        schema: Option<SchemaDigest>,
        payload_schemas: Option<std::sync::Arc<codex_native_integration::NativePayloadSchemas>>,
    ) -> io::Result<CodexGeneration> {
        if self.accepting {
            return Err(io::Error::other("backend already advertised ready"));
        }
        let next = self
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("generation exhausted"))?;
        let generation = CodexGeneration {
            service_epoch: self.epoch.clone(),
            generation: GenerationNumber::try_from(next).map_err(io::Error::other)?,
        };
        self.acp_supported = payload_schemas
            .as_ref()
            .is_some_and(|schemas| schemas.supports_server_messages());
        self.admission.activate(
            generation.clone(),
            self.backend_path.clone(),
            payload_schemas,
        )?;
        // A consumed generation is never reused, even if publication fails.
        self.generation = next;
        if let Err(error) = self.directory.publish(self.description(
            EndpointAvailability::Available { observed_at },
            Some(generation.clone()),
            schema,
        )) {
            self.admission.retire()?;
            return Err(error);
        }
        self.accepting = true;
        Ok(generation)
    }
    /// Invalidate accepting metadata without asserting that any native thread was deleted.
    pub fn unavailable(
        &mut self,
        observed_at: ObservationTimestamp,
        reason: NonEmptyText,
    ) -> io::Result<()> {
        self.admission.retire()?;
        self.acp_supported = false;
        self.accepting = false;
        self.directory.publish(self.description(
            EndpointAvailability::Unavailable {
                observed_at,
                reason,
            },
            None,
            None,
        ))?;
        self.accepting = false;
        Ok(())
    }
    fn description(
        &self,
        availability: EndpointAvailability,
        generation: Option<CodexGeneration>,
        schema_digest: Option<SchemaDigest>,
    ) -> EndpointDescription {
        let mut channels = vec![ChannelDescription::NativeCodex {
            transport: NativeCarrier::UnixWebSocket,
            path: self.native_path.clone(),
            schema_digest,
            generation,
        }];
        if self.acp_supported
            && let Some(schema_digest) = &self.acp_schema
        {
            channels.push(ChannelDescription::Acp {
                transport: communication_protocol::AcpCarrier::UnixJsonLines,
                path: self.acp_path.clone(),
                schema_digest: schema_digest.clone(),
            });
        }
        EndpointDescription {
            endpoint: self.endpoint.clone(),
            label: self.label.clone(),
            availability,
            channels,
        }
    }
}

impl Drop for BackendPublication {
    fn drop(&mut self) {
        let _retirement = self.admission.retire();
    }
}
