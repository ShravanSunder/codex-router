//! Fresh Codex session creation and effective-configuration evidence for ACP.
use crate::{AcpSchemaCatalog, McpConfiguration};
use codex_native_integration::{
    NativeConnectionError, NativeOperation, NativePayloadSchemas, NativeProtocolConnection,
};
use collaboration_protocol::{
    CodexGeneration, RouterAccess, SettingsObservation, SettingsObservationSource,
    SettingsUnavailableReason,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
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
    #[error("requested model {requested} but native runtime reported {effective}")]
    ModelMismatch {
        requested: String,
        effective: String,
    },
    #[error("requested access {requested} but native runtime reported {effective}")]
    AccessMismatch {
        requested: String,
        effective: String,
    },
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
    pub(crate) requested_access: Option<String>,
    pub(crate) settings_observation: SettingsObservation,
    pub(crate) access_route: Option<crate::ApprovalRoute>,
    pub(crate) approval_broker: std::sync::Arc<dyn crate::ApprovalBroker>,
}

struct RouterModelChoice<'a> {
    model: &'a str,
    effort: &'a str,
    fork_thread_id: Option<&'a str>,
    access: RouterAccess,
    created_by: collaboration_protocol::SessionRef,
    approver: collaboration_protocol::SessionRef,
    scratch_path: &'a str,
    root_message_id: Option<&'a str>,
}

fn router_model_choice(params: &Value) -> Result<RouterModelChoice<'_>, SessionSetupError> {
    let router = params
        .pointer("/_meta/codexRouter")
        .and_then(Value::as_object)
        .ok_or(SessionSetupError::InvalidParameters)?;
    let model = router
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_whitespace))
        .ok_or(SessionSetupError::InvalidParameters)?;
    let effort = router
        .get("effort")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_whitespace))
        .ok_or(SessionSetupError::InvalidParameters)?;
    let fork_thread_id = router
        .get("forkThreadId")
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or(SessionSetupError::InvalidParameters)
        })
        .transpose()?;
    let access = match router
        .get("access")
        .and_then(Value::as_str)
        .ok_or(SessionSetupError::InvalidParameters)?
    {
        "write-restricted" => RouterAccess::WriteRestricted,
        "workspace-write" => RouterAccess::WorkspaceWrite,
        _ => return Err(SessionSetupError::InvalidParameters),
    };
    let created_by = serde_json::from_value(
        router
            .get("createdBy")
            .cloned()
            .ok_or(SessionSetupError::InvalidParameters)?,
    )
    .map_err(|_| SessionSetupError::InvalidParameters)?;
    let approver = serde_json::from_value(
        router
            .get("approver")
            .cloned()
            .ok_or(SessionSetupError::InvalidParameters)?,
    )
    .map_err(|_| SessionSetupError::InvalidParameters)?;
    let scratch_path = router
        .get("scratchPath")
        .and_then(Value::as_str)
        .ok_or(SessionSetupError::InvalidParameters)?;
    let scratch_scope = router
        .get("scratchScope")
        .and_then(Value::as_str)
        .ok_or(SessionSetupError::InvalidParameters)?;
    let root_message_id = router.get("rootMessageId").and_then(Value::as_str);
    validate_scratch(scratch_path, scratch_scope, root_message_id)?;
    Ok(RouterModelChoice {
        model,
        effort,
        fork_thread_id,
        access,
        created_by,
        approver,
        scratch_path,
        root_message_id,
    })
}
pub struct SessionSetupInputs {
    pub connection: NativeProtocolConnection,
    pub schemas: Arc<NativePayloadSchemas>,
    pub generation: CodexGeneration,
    pub params: Value,
    pub approval_broker: std::sync::Arc<dyn crate::ApprovalBroker>,
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
        let choice = router_model_choice(&inputs.params)?;
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
        let mut native = json!({
            "cwd":cwd,
            "experimentalRawEvents":false,
            "model":choice.model,
            "allowProviderModelFallback":false,
            "threadSource":"user",
            "config":{"model_reasoning_effort":choice.effort}
        });
        let fields = native
            .as_object_mut()
            .ok_or(SessionSetupError::InvalidParameters)?;
        let profile = match choice.access {
            RouterAccess::WriteRestricted => "router-write-restricted",
            RouterAccess::WorkspaceWrite => "router-workspace-write",
        };
        fields.insert("permissions".into(), json!(profile));
        if let Some(config) = configuration.native_overrides() {
            let target = fields
                .get_mut("config")
                .and_then(Value::as_object_mut)
                .ok_or(SessionSetupError::InvalidParameters)?;
            let values = config
                .as_object()
                .ok_or(SessionSetupError::InvalidParameters)?;
            target.extend(values.clone());
        }
        let config = fields
            .get_mut("config")
            .and_then(Value::as_object_mut)
            .ok_or(SessionSetupError::InvalidParameters)?;
        config.insert("default_permissions".into(), json!(profile));
        let mut filesystem = serde_json::Map::new();
        filesystem.insert(choice.scratch_path.to_owned(), json!("write"));
        if choice.access == RouterAccess::WriteRestricted {
            filesystem.insert(
                expected_cwd.join("tmp").to_string_lossy().into_owned(),
                json!("write"),
            );
            filesystem.insert(
                expected_cwd.join("docs/wip").to_string_lossy().into_owned(),
                json!("write"),
            );
        }
        config.insert(format!("permissions.{profile}"), json!({
            "extends": if choice.access == RouterAccess::WriteRestricted { ":read-only" } else { ":workspace" },
            "filesystem": filesystem
        }));
        if !directories.is_empty() {
            fields.insert("runtimeWorkspaceRoots".into(), json!(directories));
        }
        let mut connection = inputs.connection;
        let operation = if choice.fork_thread_id.is_some() {
            NativeOperation::ForkThread
        } else {
            NativeOperation::StartThread
        };
        if let Some(fork_thread_id) = choice.fork_thread_id {
            fields.insert("threadId".into(), json!(fork_thread_id));
        }
        let result = connection
            .request_validated(&inputs.schemas, operation, native)
            .await
            .map_err(map_native_failure)?;
        if result.get("model").and_then(Value::as_str) != Some(choice.model) {
            return Err(SessionSetupError::ModelMismatch {
                requested: choice.model.to_owned(),
                effective: result
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("<missing>")
                    .to_owned(),
            });
        }
        let settings_observation = observe_settings(
            &result,
            if choice.fork_thread_id.is_some() {
                SettingsObservationSource::ThreadFork
            } else {
                SettingsObservationSource::ThreadStart
            },
            Some(choice.access),
        );
        validate_observed_settings(
            &result,
            choice.access,
            &expected_cwd,
            Path::new(choice.scratch_path),
            profile,
        )?;
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
        let access_route = crate::ApprovalRoute {
            thread_id: session_id.clone(),
            created_by: choice.created_by,
            approver: choice.approver,
            access: choice.access,
            scratch_path: choice.scratch_path.to_owned(),
            root_message_id: choice.root_message_id.map(str::to_owned),
        };
        inputs
            .approval_broker
            .register_route(access_route.clone())
            .await
            .map_err(|_| SessionSetupError::ConfigurationMismatch)?;
        Ok(Self {
            session_id,
            generation: inputs.generation,
            configuration: Some(configuration),
            connection,
            schemas: inputs.schemas,
            requested_access: Some(access_name(choice.access).to_owned()),
            settings_observation,
            access_route: Some(access_route),
            approval_broker: inputs.approval_broker,
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
        let route = inputs
            .approval_broker
            .route(&session_id)
            .await
            .map_err(|_| SessionSetupError::Unavailable)?;
        let mut session = Self {
            session_id,
            generation: inputs.generation.clone(),
            configuration: None,
            connection: inputs.connection,
            schemas: inputs.schemas,
            requested_access: route
                .as_ref()
                .map(|route| access_name(route.access).to_owned()),
            settings_observation: unavailable_settings(
                SettingsUnavailableReason::NotObservedBeforeResume,
            ),
            access_route: route,
            approval_broker: inputs.approval_broker,
        };
        let response = session
            .resume_with_receipt(catalog, &inputs.generation, &inputs.params)
            .await?;
        session.settings_observation = observe_settings(
            &response,
            SettingsObservationSource::ThreadResume,
            session.access_route.as_ref().map(|route| route.access),
        );
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
                resume_parameters(session_id, &requested_cwd, self.access_route.as_ref()),
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
        if let Some(route) = self.access_route.as_ref() {
            let profile = profile_name(route.access);
            validate_observed_settings(
                &result,
                route.access,
                &requested_cwd,
                Path::new(&route.scratch_path),
                profile,
            )?;
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

fn resume_parameters(session_id: &str, cwd: &Path, route: Option<&crate::ApprovalRoute>) -> Value {
    let Some(route) = route else {
        return json!({"threadId":session_id});
    };
    let profile = profile_name(route.access);
    let mut filesystem = serde_json::Map::new();
    filesystem.insert(route.scratch_path.clone(), json!("write"));
    if route.access == RouterAccess::WriteRestricted {
        filesystem.insert(
            cwd.join("tmp").to_string_lossy().into_owned(),
            json!("write"),
        );
        filesystem.insert(
            cwd.join("docs/wip").to_string_lossy().into_owned(),
            json!("write"),
        );
    }
    json!({
        "threadId":session_id,
        "permissions":profile,
        "config":{
            "default_permissions":profile,
            format!("permissions.{profile}"):{
                "extends":if route.access == RouterAccess::WriteRestricted { ":read-only" } else { ":workspace" },
                "filesystem":filesystem
            }
        }
    })
}

fn observe_settings(
    response: &Value,
    source: SettingsObservationSource,
    router_access: Option<RouterAccess>,
) -> SettingsObservation {
    let sandbox = response.get("sandbox").cloned();
    let approval_policy = response.get("approvalPolicy").cloned();
    let approvals_reviewer = response.get("approvalsReviewer").cloned();
    let permission_profile = response.get("activePermissionProfile").cloned();
    if sandbox.is_none()
        && approval_policy.is_none()
        && approvals_reviewer.is_none()
        && permission_profile.is_none()
    {
        return unavailable_settings(SettingsUnavailableReason::NativeResponseOmittedSettings);
    }
    SettingsObservation::Observed {
        source,
        observed_at: chrono::Utc::now().to_rfc3339(),
        router_access,
        native_sandbox: sandbox,
        permission_profile,
        approval_policy,
        approvals_reviewer,
    }
}

fn unavailable_settings(reason: SettingsUnavailableReason) -> SettingsObservation {
    SettingsObservation::Unavailable { reason }
}

const fn access_name(access: RouterAccess) -> &'static str {
    match access {
        RouterAccess::WriteRestricted => "write-restricted",
        RouterAccess::WorkspaceWrite => "workspace-write",
    }
}

const fn profile_name(access: RouterAccess) -> &'static str {
    match access {
        RouterAccess::WriteRestricted => "router-write-restricted",
        RouterAccess::WorkspaceWrite => "router-workspace-write",
    }
}

fn validate_scratch(
    scratch_path: &str,
    scratch_scope: &str,
    root_message_id: Option<&str>,
) -> Result<(), SessionSetupError> {
    let scope_valid = if let Some(root) = root_message_id {
        collaboration_protocol::UuidIdentity::try_from(root.to_owned()).is_ok()
            && scratch_scope == root
    } else {
        scratch_scope
            .strip_prefix("session-")
            .is_some_and(|id| collaboration_protocol::UuidIdentity::try_from(id.to_owned()).is_ok())
    };
    let path = Path::new(scratch_path);
    if !scope_valid
        || !path.is_absolute()
        || path.file_name().and_then(|name| name.to_str()) != Some(scratch_scope)
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some("scratch")
    {
        return Err(SessionSetupError::InvalidParameters);
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| SessionSetupError::InvalidParameters)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(SessionSetupError::InvalidParameters);
    }
    Ok(())
}

fn validate_observed_settings(
    response: &Value,
    access: RouterAccess,
    cwd: &Path,
    scratch: &Path,
    profile: &str,
) -> Result<(), SessionSetupError> {
    let profile_id = response
        .pointer("/activePermissionProfile/id")
        .and_then(Value::as_str)
        .ok_or(SessionSetupError::ConfigurationMismatch)?;
    let roots = response
        .pointer("/sandbox/writableRoots")
        .and_then(Value::as_array)
        .ok_or(SessionSetupError::ConfigurationMismatch)?;
    let actual = roots
        .iter()
        .map(|root| {
            root.as_str()
                .map(str::to_owned)
                .ok_or(SessionSetupError::ConfigurationMismatch)
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    let mut expected = std::collections::BTreeSet::from([scratch
        .to_str()
        .ok_or(SessionSetupError::ConfigurationMismatch)?
        .to_owned()]);
    match access {
        RouterAccess::WriteRestricted => {
            expected.insert(cwd.join("tmp").to_string_lossy().into_owned());
            expected.insert(cwd.join("docs/wip").to_string_lossy().into_owned());
        }
        RouterAccess::WorkspaceWrite => {
            expected.insert(
                cwd.to_str()
                    .ok_or(SessionSetupError::ConfigurationMismatch)?
                    .to_owned(),
            );
        }
    }
    if profile_id != profile || actual != expected {
        return Err(SessionSetupError::AccessMismatch {
            requested: access_name(access).to_owned(),
            effective: format!("profile={profile_id}, roots={actual:?}"),
        });
    }
    Ok(())
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

#[cfg(test)]
mod access_validation_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn scratch_metadata_is_required_private_and_scope_bound() {
        let scope = "session-00000000-0000-4000-8000-000000000099";
        let path = std::env::temp_dir().join("scratch").join(scope);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_scratch(path.to_str().unwrap(), scope, None).is_ok());
        assert!(validate_scratch("/tmp", scope, None).is_err());
        assert!(
            validate_scratch(
                path.to_str().unwrap(),
                "session-00000000-0000-4000-8000-000000000098",
                None
            )
            .is_err()
        );
        assert!(
            validate_scratch(
                path.to_str().unwrap(),
                scope,
                Some("00000000-0000-4000-8000-000000000099")
            )
            .is_err()
        );
    }

    #[test]
    fn managed_setup_refuses_missing_or_incompatible_native_settings() {
        let cwd = Path::new("/repo");
        let scratch = Path::new("/owner/scratch/root");
        assert!(
            validate_observed_settings(
                &json!({}),
                RouterAccess::WriteRestricted,
                cwd,
                scratch,
                "router-write-restricted"
            )
            .is_err()
        );
        let wrong = json!({
            "activePermissionProfile":{"id":"router-write-restricted"},
            "sandbox":{"writableRoots":["/owner/scratch/root","/repo/tmp"]}
        });
        assert!(
            validate_observed_settings(
                &wrong,
                RouterAccess::WriteRestricted,
                cwd,
                scratch,
                "router-write-restricted"
            )
            .is_err()
        );
    }
}
