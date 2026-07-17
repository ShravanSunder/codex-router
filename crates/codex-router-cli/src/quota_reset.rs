//! Integrated, live-only usage-limit reset workflow.

use thiserror::Error;

pub(crate) mod credential_authority;
pub(crate) mod provider_protocol;
#[cfg(feature = "quota-reset-test-harness")]
mod quota_reset_loopback_harness;
mod quota_reset_session_composition;
pub(crate) mod reset_commit_service;
pub(crate) mod reset_credit_policy;
pub(crate) mod reset_session_supervisor;
pub(crate) mod reset_workflow_reducer;

#[cfg(feature = "quota-reset-test-harness")]
pub(crate) use quota_reset_loopback_harness::run_quota_reset_test_harness_with_io;
pub(crate) use quota_reset_session_composition::FixedOriginInteractiveResetSessionFactory;
#[cfg(test)]
pub(crate) use quota_reset_session_composition::InteractiveResetSession;
pub(crate) use quota_reset_session_composition::InteractiveResetSessionFactory;
#[cfg(feature = "quota-reset-test-harness")]
pub(crate) use quota_reset_session_composition::LoopbackInteractiveResetSessionFactory;

/// Sanitized reset-provider composition and protocol failure.
#[derive(Debug, Error)]
pub enum QuotaResetError {
    #[error("quota reset provider request failed: {message}")]
    Request { message: String },
    #[error("quota reset provider returned HTTP {status}")]
    Status { status: u16 },
    #[error("quota reset provider response was unusable: {message}")]
    Response { message: String },
}
