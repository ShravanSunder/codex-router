//! Attach held or persisted Codex sessions while preserving the original binding evidence.
use super::*;

impl AcpSessionBinding {
    pub async fn adopt_unmaterialized(
        &self,
        catalog: &mut AcpSchemaCatalog,
        generation: &CodexGeneration,
        params: &Value,
    ) -> Result<(), SessionSetupError> {
        if !self.is_unmaterialized()
            || !catalog
                .validate("LoadSessionRequest", params)
                .map_err(|_| SessionSetupError::SchemaUnavailable)?
        {
            return Err(SessionSetupError::InvalidParameters);
        }
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let servers = params
            .get("mcpServers")
            .and_then(Value::as_array)
            .ok_or(SessionSetupError::InvalidParameters)?;
        let configuration = McpConfiguration::parse(catalog, servers)
            .map_err(|_| SessionSetupError::InvalidParameters)?;
        if !self.accepts_configuration(generation, session_id, &configuration)
            || normalized_directory(cwd).await? != self.working_directory
        {
            return Err(SessionSetupError::ConfigurationMismatch);
        }
        Ok(())
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
        )
        .await?;
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
            if normalized_directory(effective).await? != requested_cwd {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeThreadActivity {
    Idle,
    Active,
    Invalid,
}

pub(crate) fn native_thread_activity(resume_response: &Value) -> NativeThreadActivity {
    let Some(thread) = resume_response.get("thread") else {
        return NativeThreadActivity::Invalid;
    };
    let Some(status) = thread.pointer("/status/type").and_then(Value::as_str) else {
        return NativeThreadActivity::Invalid;
    };
    match status {
        "active" => NativeThreadActivity::Active,
        "idle" => {
            let has_active_turn =
                thread
                    .get("turns")
                    .and_then(Value::as_array)
                    .is_some_and(|turns| {
                        turns.iter().any(|turn| {
                            turn.get("status").and_then(Value::as_str) == Some("inProgress")
                        })
                    });
            if has_active_turn {
                NativeThreadActivity::Active
            } else {
                NativeThreadActivity::Idle
            }
        }
        "notLoaded" | "systemError" => NativeThreadActivity::Invalid,
        _ => NativeThreadActivity::Invalid,
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
            format!("permissions.{profile}.extends"):if route.access == RouterAccess::WriteRestricted { ":read-only" } else { ":workspace" },
            format!("permissions.{profile}.filesystem"):filesystem
        }
    })
}

#[cfg(test)]
mod activity_tests {
    use super::{NativeThreadActivity, native_thread_activity};
    use serde_json::json;

    #[test]
    fn active_resume_state_blocks_session_attachment() {
        assert_eq!(
            native_thread_activity(&json!({"thread":{"status":{"type":"active"},"turns":[]}})),
            NativeThreadActivity::Active
        );
        assert_eq!(
            native_thread_activity(
                &json!({"thread":{"status":{"type":"idle"},"turns":[{"status":"inProgress"}]}})
            ),
            NativeThreadActivity::Active
        );
        assert_eq!(
            native_thread_activity(
                &json!({"thread":{"status":{"type":"idle"},"turns":[{"status":"completed"}]}})
            ),
            NativeThreadActivity::Idle
        );
        assert_eq!(
            native_thread_activity(&json!({"thread":{"turns":[]}})),
            NativeThreadActivity::Invalid
        );
        assert_eq!(
            native_thread_activity(
                &json!({"thread":{"status":{"type":"future-state"},"turns":[]}})
            ),
            NativeThreadActivity::Invalid
        );
    }
}
