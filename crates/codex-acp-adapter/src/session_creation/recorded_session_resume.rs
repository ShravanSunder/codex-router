//! Prepare and verify native attachment from the existing recorded Router policy.
use super::{RouterSessionProfile, SessionSetupError, normalized_directory};
use crate::ApprovalRoute;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Opaque projection shared by native callers; the ACP profile owner defines the grants.
pub struct RecordedSessionResume {
    thread_id: String,
    working_directory: PathBuf,
    profile: RouterSessionProfile,
}

impl RecordedSessionResume {
    pub async fn prepare(
        route: &ApprovalRoute,
        thread_id: &str,
        working_directory: &str,
    ) -> Result<Self, SessionSetupError> {
        if route.thread_id != thread_id
            || thread_id.is_empty()
            || !Path::new(&route.scratch_path).is_absolute()
            || route.scratch_path.contains('\0')
        {
            return Err(SessionSetupError::ConfigurationMismatch);
        }
        let working_directory = normalized_directory(working_directory).await?;
        let profile = RouterSessionProfile::for_session(
            route.access,
            &working_directory,
            Path::new(&route.scratch_path),
        )
        .await?;
        Ok(Self {
            thread_id: thread_id.to_owned(),
            working_directory,
            profile,
        })
    }

    #[must_use]
    pub fn native_parameters(&self) -> Value {
        json!({
            "threadId":self.thread_id,
            "permissions":self.profile.name(),
            "config":self.profile.native_config(),
            "excludeTurns":true
        })
    }

    /// Rejoin loaded state without overrides that could make native rebuild its cache.
    /// A concurrent unload can still produce a cold response; validate it before input.
    #[must_use]
    pub fn native_parameters_for_loaded_thread(&self) -> Value {
        json!({"threadId":self.thread_id,"excludeTurns":true})
    }

    /// A successful RPC can still narrow access. Validate before submitting any turn.
    pub async fn validate_response(&self, response: &Value) -> Result<(), SessionSetupError> {
        if response.pointer("/thread/id").and_then(Value::as_str) != Some(&self.thread_id) {
            return Err(SessionSetupError::OutcomeUnknown);
        }
        for pointer in ["/cwd", "/thread/cwd"] {
            let directory = response
                .pointer(pointer)
                .and_then(Value::as_str)
                .ok_or(SessionSetupError::OutcomeUnknown)?;
            if normalized_directory(directory).await? != self.working_directory {
                return Err(SessionSetupError::ConfigurationMismatch);
            }
        }
        self.profile.validate_observed(response)
    }
}
