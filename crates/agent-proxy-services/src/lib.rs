//! Proxy role behavior and its credential/refresh producers.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::panic_in_result_fn))]
pub mod credential_runtime;
mod credential_store_open;
pub mod credential_upkeep_worker;
pub mod quota;
pub mod token_reload_watcher;
/// Existing shared refresh cadence, also consumed by standalone CLI commands.
pub const DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 180;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

mod proxy_preparation_error;
mod proxy_role_config;
mod proxy_role_lifecycle;
mod proxy_role_preparation;
mod proxy_role_runtime;
pub use proxy_role_lifecycle::{ProxyDeactivationReceipt, ProxyDrainCompletion};
mod proxy_secret_preparation;
pub use proxy_preparation_error::ProxyPreparationError;
pub use proxy_role_config::{ProxyLocalTokenPolicy, ProxyQuotaRefreshPolicy, ProxyRoleConfig};
pub use proxy_role_preparation::{PreparedProxyRoleRuntime, ProxyPreparedStateSchema};
pub use proxy_role_runtime::{ProxyActivationError, ProxyRoleRuntime};

#[cfg(test)]
mod proxy_role_preparation_tests;
#[cfg(test)]
mod proxy_role_test_fixtures;

#[cfg(any(test, feature = "test-support", feature = "keychain-test-support"))]
mod proxy_keychain_fixture;

#[cfg(test)]
mod proxy_secret_preparation_tests;

#[cfg(test)]
mod proxy_fresh_bootstrap_tests;

#[cfg(test)]
mod proxy_drain_test_fixtures;
#[cfg(test)]
mod proxy_drain_tests;
