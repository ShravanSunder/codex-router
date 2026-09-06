//! Connection-local admission rules independent of operation effects.
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AdmissionError {
    #[error("request ID must contain 1 to 128 UTF-8 bytes")]
    InvalidId,
    #[error("request ID was already used on this connection")]
    ReusedId,
    #[error("Control initialization has not completed")]
    NotInitialized,
    #[error("Control initialization was already requested")]
    AlreadyInitialized,
    #[error("Control connection request budget exceeded")]
    Overloaded,
}

/// Tracks retired IDs and in-flight admission, never native delivery or retries.
#[derive(Default)]
pub struct ControlAdmission {
    initialized: bool,
    initialization_requested: bool,
    retired_ids: HashSet<String>,
    pending_ids: HashSet<String>,
}
impl ControlAdmission {
    pub fn admit(&mut self, id: &str, method: &str) -> Result<(), AdmissionError> {
        if id.is_empty() || id.len() > 128 {
            return Err(AdmissionError::InvalidId);
        }
        if self.retired_ids.contains(id) {
            return Err(AdmissionError::ReusedId);
        }
        if self.retired_ids.len() >= 65_536 {
            return Err(AdmissionError::Overloaded);
        }
        // Even a rejected request ID has been used on this connection.
        self.retired_ids.insert(id.to_owned());
        if self.pending_ids.len() >= 64 {
            return Err(AdmissionError::Overloaded);
        }
        if method == "control/initialize" {
            if self.initialization_requested {
                return Err(AdmissionError::AlreadyInitialized);
            }
            self.initialization_requested = true;
        } else if !self.initialized {
            return Err(AdmissionError::NotInitialized);
        }
        self.pending_ids.insert(id.to_owned());
        Ok(())
    }

    /// Called only after successful version/schema negotiation by the connection owner.
    pub fn initialized(&mut self) {
        if self.initialization_requested {
            self.initialized = true;
        }
    }

    /// Releases pending capacity while preserving the retired request identity.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn complete(&mut self, id: &str) {
        self.pending_ids.remove(id);
    }
}
