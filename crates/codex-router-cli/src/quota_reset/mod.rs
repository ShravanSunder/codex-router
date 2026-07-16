//! Integrated, live-only usage-limit reset workflow.

use thiserror::Error;

mod composition;
pub(crate) mod credentials;
pub(crate) mod domain;
pub(crate) mod provider;
pub(crate) mod service;
pub(crate) mod supervisor;
pub(crate) mod workflow;

pub(crate) use composition::compose_production_reset_session;

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
