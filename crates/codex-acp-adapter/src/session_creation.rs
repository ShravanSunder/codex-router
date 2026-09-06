//! Fresh Codex session creation and effective-configuration evidence for ACP.
use crate::{AcpSchemaCatalog, McpConfiguration};
use codex_native_integration::{
    NativeConnectionError, NativeOperation, NativePayloadSchemas, NativeProtocolConnection,
};
use communication_protocol::CodexGeneration;
use serde_json::{Value, json};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, thiserror::Error)]
pub enum SessionSetupError {
    #[error("native cancellation remains unresolved")]
    CancellationUnresolved,
    #[error("invalid ACP session parameters")]
    InvalidParameters,
    #[error("native session effective configuration differs from requested configuration")]
    ConfigurationMismatch,
    #[error("native session creation outcome unknown")]
    OutcomeUnknown,
    #[error("native session request rejected")]
    NativeRejected,
    #[error("session connection unavailable")]
    Unavailable,
    #[error("session schema unavailable")]
    SchemaUnavailable,
}
/// Retains one connection and a receipt minted only by successful fresh thread/start.
pub struct AcpSessionBinding {
    pub(crate) session_id: String,
    pub(crate) generation: CodexGeneration,
    configuration: Option<McpConfiguration>,
    pub(crate) connection: NativeProtocolConnection,
    pub(crate) schemas: Arc<NativePayloadSchemas>,
}
pub struct SessionSetupInputs {
    pub connection: NativeProtocolConnection,
    pub schemas: Arc<NativePayloadSchemas>,
    pub generation: CodexGeneration,
    pub params: Value,
}
impl AcpSessionBinding {
    pub async fn create(
        catalog: &mut AcpSchemaCatalog,
        inputs: SessionSetupInputs,
    ) -> Result<Self, SessionSetupError> {
        if !catalog
            .validate("NewSessionRequest", &inputs.params)
            .map_err(|_| SessionSetupError::SchemaUnavailable)?
        {
            return Err(SessionSetupError::InvalidParameters);
        }
        let cwd = inputs
            .params
            .get("cwd")
            .and_then(Value::as_str)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let expected_cwd = normalized_directory(cwd)?;
        let servers = inputs
            .params
            .get("mcpServers")
            .and_then(Value::as_array)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let configuration = McpConfiguration::parse(catalog, servers)
            .map_err(|_| SessionSetupError::InvalidParameters)?;
        let directories = match inputs.params.get("additionalDirectories") {
            None => Vec::new(),
            Some(value) => value
                .as_array()
                .ok_or(SessionSetupError::InvalidParameters)?
                .iter()
                .map(|value| {
                    normalized_directory(
                        value.as_str().ok_or(SessionSetupError::InvalidParameters)?,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        let mut native = json!({"cwd":cwd,"experimentalRawEvents":false});
        let fields = native
            .as_object_mut()
            .ok_or(SessionSetupError::InvalidParameters)?;
        if let Some(config) = configuration.native_overrides() {
            fields.insert("config".into(), config);
        }
        if !directories.is_empty() {
            fields.insert("runtimeWorkspaceRoots".into(), json!(directories));
        }
        let mut connection = inputs.connection;
        let result = connection
            .request_validated(&inputs.schemas, NativeOperation::StartThread, native)
            .await
            .map_err(map_native_failure)?;
        let effective_cwd = result
            .get("cwd")
            .and_then(Value::as_str)
            .ok_or(SessionSetupError::OutcomeUnknown)?;
        if normalized_directory(effective_cwd)? != expected_cwd {
            return Err(SessionSetupError::ConfigurationMismatch);
        }
        let thread = result
            .get("thread")
            .ok_or(SessionSetupError::OutcomeUnknown)?;
        if normalized_directory(
            thread
                .get("cwd")
                .and_then(Value::as_str)
                .ok_or(SessionSetupError::OutcomeUnknown)?,
        )? != expected_cwd
        {
            return Err(SessionSetupError::ConfigurationMismatch);
        }
        if !directories.is_empty() {
            let actual = result
                .get("runtimeWorkspaceRoots")
                .and_then(Value::as_array)
                .ok_or(SessionSetupError::ConfigurationMismatch)?
                .iter()
                .map(|value| {
                    normalized_directory(
                        value
                            .as_str()
                            .ok_or(SessionSetupError::ConfigurationMismatch)?,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if actual != directories {
                return Err(SessionSetupError::ConfigurationMismatch);
            }
        }
        let session_id = thread
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(SessionSetupError::OutcomeUnknown)?
            .to_owned();
        Ok(Self {
            session_id,
            generation: inputs.generation,
            configuration: Some(configuration),
            connection,
            schemas: inputs.schemas,
        })
    }
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    #[must_use]
    pub fn new_session_result(&self) -> Value {
        json!({"sessionId":self.session_id})
    }
    /// Empty configuration permits attachment to an existing native session without
    /// claiming that this adapter installed its configuration.
    pub async fn load_existing(
        catalog: &mut AcpSchemaCatalog,
        inputs: SessionSetupInputs,
    ) -> Result<(Self, Vec<Value>), SessionSetupError> {
        Self::load_existing_guarded(catalog, inputs, None).await
    }
    pub(crate) async fn load_existing_guarded(
        catalog: &mut AcpSchemaCatalog,
        inputs: SessionSetupInputs,
        barrier: Option<&crate::CancellationBarrier>,
    ) -> Result<(Self, Vec<Value>), SessionSetupError> {
        if !catalog
            .validate("LoadSessionRequest", &inputs.params)
            .map_err(|_| SessionSetupError::SchemaUnavailable)?
        {
            return Err(SessionSetupError::InvalidParameters);
        }
        let servers = inputs
            .params
            .get("mcpServers")
            .and_then(Value::as_array)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let requested = McpConfiguration::parse(catalog, servers)
            .map_err(|_| SessionSetupError::InvalidParameters)?;
        if !requested.is_empty() {
            return Err(SessionSetupError::ConfigurationMismatch);
        }
        let session_id = inputs
            .params
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(SessionSetupError::InvalidParameters)?
            .to_owned();
        let mut session = Self {
            session_id,
            generation: inputs.generation.clone(),
            configuration: None,
            connection: inputs.connection,
            schemas: inputs.schemas,
        };
        let response = session
            .resume_with_receipt(catalog, &inputs.generation, &inputs.params)
            .await?;
        if barrier.is_some_and(|barrier| !barrier.resolved_by_resume(&response)) {
            return Err(SessionSetupError::CancellationUnresolved);
        }
        let history = crate::project_history(catalog, session.session_id(), &response)
            .map_err(|_| SessionSetupError::OutcomeUnknown)?;
        Ok((session, history))
    }
    /// Reattaches only this known session. Successful resume never mints new configuration evidence.
    /// Caller must project historical updates before returning ACP LoadSessionResponse.
    pub async fn resume_with_receipt(
        &mut self,
        catalog: &mut AcpSchemaCatalog,
        generation: &CodexGeneration,
        params: &Value,
    ) -> Result<Value, SessionSetupError> {
        if !catalog
            .validate("LoadSessionRequest", params)
            .map_err(|_| SessionSetupError::SchemaUnavailable)?
        {
            return Err(SessionSetupError::InvalidParameters);
        }
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let requested_cwd = normalized_directory(
            params
                .get("cwd")
                .and_then(Value::as_str)
                .ok_or(SessionSetupError::InvalidParameters)?,
        )?;
        let servers = params
            .get("mcpServers")
            .and_then(Value::as_array)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let configuration = McpConfiguration::parse(catalog, servers)
            .map_err(|_| SessionSetupError::InvalidParameters)?;
        if !self.accepts_configuration(generation, session_id, &configuration) {
            return Err(SessionSetupError::ConfigurationMismatch);
        }
        let result = self
            .connection
            .request_validated(
                &self.schemas,
                NativeOperation::ResumeThread,
                json!({"threadId":session_id}),
            )
            .await
            .map_err(map_native_failure)?;
        let thread = result
            .get("thread")
            .ok_or(SessionSetupError::OutcomeUnknown)?;
        if thread.get("id").and_then(Value::as_str) != Some(session_id) {
            return Err(SessionSetupError::OutcomeUnknown);
        }
        for cwd in [result.get("cwd"), thread.get("cwd")] {
            let effective = cwd
                .and_then(Value::as_str)
                .ok_or(SessionSetupError::OutcomeUnknown)?;
            if normalized_directory(effective)? != requested_cwd {
                return Err(SessionSetupError::ConfigurationMismatch);
            }
        }
        Ok(result)
    }
    /// Empty load configuration adds nothing; a nonempty load must match fresh-new evidence.
    #[must_use]
    pub fn accepts_configuration(
        &self,
        generation: &CodexGeneration,
        session_id: &str,
        configuration: &McpConfiguration,
    ) -> bool {
        self.generation == *generation
            && self.session_id == session_id
            && (configuration.is_empty() || self.configuration.as_ref() == Some(configuration))
    }
    pub fn into_connection(self) -> (NativeProtocolConnection, Arc<NativePayloadSchemas>) {
        (self.connection, self.schemas)
    }
}
fn normalized_directory(value: &str) -> Result<PathBuf, SessionSetupError> {
    let path = Path::new(value);
    if !path.is_absolute() || value.contains('\0') {
        return Err(SessionSetupError::InvalidParameters);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}
fn map_native_failure(error: NativeConnectionError) -> SessionSetupError {
    match error {
        NativeConnectionError::Unavailable => SessionSetupError::Unavailable,
        NativeConnectionError::InvalidInput => SessionSetupError::InvalidParameters,
        NativeConnectionError::Rejected { .. } => SessionSetupError::NativeRejected,
        _ => SessionSetupError::OutcomeUnknown,
    }
}
