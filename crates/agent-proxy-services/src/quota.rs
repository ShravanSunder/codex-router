use crate::credential_runtime::{AsyncCliCredentialResolver, AsyncProviderCredentialResolver};
use codex_router_auth::live_quota::{UsageResponse, WindowPair, reset_credits_url, usage_url};
use codex_router_core::{
    credit_usage::CreditProviderObservation,
    ids::AccountId,
    redaction::{SecretString, safe_account_label},
};
use codex_router_proxy::websocket::WebSocketQuotaFloorNotifier;
use codex_router_selection::burn_down::{
    V1_SHORT_WINDOW_SECONDS, V1_WEEKLY_WINDOW_SECONDS, weekly_quota_switch_at_basis_points,
};
use codex_router_state::{
    account::{AccountRecord, AccountStatus},
    credit_store::CreditRefreshAttempt,
    quota_snapshot::{
        PersistedQuotaHistoryObservation, PersistedQuotaSnapshot, PersistedSelectorQuotaWindow,
        QuotaHistoryRefreshOutcome, QuotaRefreshErrorClass, QuotaSnapshotSource,
        SelectorQuotaWindowStatus,
    },
    sqlite::{AsyncSqliteStateStore, StateStoreError},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
mod refresh_error;
pub use refresh_error::QuotaRefreshError;
mod claude_quota_fetcher;
mod quota_account_refresh;
mod quota_background_refresh_worker;
mod quota_refresh_helpers;
mod quota_refresh_history;
mod quota_refresh_provider;
mod quota_refresh_service;
use quota_refresh_helpers::*;
mod quota_claude_refresh;
mod quota_openai_refresh;
pub mod refresh_telemetry;
pub use quota_background_refresh_worker::*;
use quota_refresh_history::*;
pub use quota_refresh_provider::*;
pub use quota_refresh_service::*;
use refresh_telemetry::*;
const DEFAULT_ROUTE_BANDS: &[&str] = &["responses", "models"];
pub const USER_QUOTA_ROUTE_BAND: &str = "responses";
pub const DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS: u64 = 360;
pub const ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS: u64 = 7_200;
fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
