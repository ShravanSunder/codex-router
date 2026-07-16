//! Integrated, live-only usage-limit reset workflow.

use thiserror::Error;

mod composition;
pub(crate) mod credentials;
pub(crate) mod domain;
pub(crate) mod provider;
pub(crate) mod service;
pub(crate) mod supervisor;
#[cfg(feature = "quota-reset-test-harness")]
mod test_harness;
pub(crate) mod workflow;

pub(crate) use composition::FixedOriginInteractiveResetSessionFactory;
#[cfg(test)]
pub(crate) use composition::InteractiveResetSession;
pub(crate) use composition::InteractiveResetSessionFactory;
#[cfg(feature = "quota-reset-test-harness")]
pub(crate) use composition::LoopbackInteractiveResetSessionFactory;
#[cfg(feature = "quota-reset-test-harness")]
pub(crate) use test_harness::run_quota_reset_test_harness_with_io;

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
