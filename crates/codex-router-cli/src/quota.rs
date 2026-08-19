//! Quota command glue for persisted router-owned quota state.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_auth::live_quota::UsageResponse;
use codex_router_auth::live_quota::WindowPair;
use codex_router_auth::live_quota::reset_credits_url;
use codex_router_auth::live_quota::usage_url;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_core::redaction::safe_account_label;
use codex_router_core::routes::RouteBand;
use codex_router_proxy::websocket::WebSocketQuotaFloorNotifier;
use codex_router_selection::burn_down::AccountAvailability;
use codex_router_selection::burn_down::BurnDownAccountAssessment;
use codex_router_selection::burn_down::BurnDownAccountInput;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
use codex_router_selection::burn_down::LimitingWindow;
use codex_router_selection::burn_down::QuotaEvidenceFreshness;
use codex_router_selection::burn_down::QuotaEvidenceReason;
use codex_router_selection::burn_down::QuotaWindowFact;
use codex_router_selection::burn_down::QuotaWindowStatus;
use codex_router_selection::burn_down::RoutingExclusion;
use codex_router_selection::burn_down::RoutingReason;
use codex_router_selection::burn_down::SelectedPool;
use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use codex_router_selection::burn_down::assess_route_band;
use codex_router_selection::run_rate::QuotaRunRateConfidence;
use codex_router_selection::run_rate::QuotaRunRateEstimate;
use codex_router_selection::run_rate::QuotaRunRateEstimator;
use codex_router_selection::run_rate::QuotaRunRateObservation;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaHistoryRefreshOutcome;
use codex_router_state::quota_snapshot::QuotaRefreshErrorClass;
use codex_router_state::quota_snapshot::QuotaRefreshStatusSource;
use codex_router_state::quota_snapshot::QuotaRefreshStatusView;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::selection_projection::project_route_band_selection_inputs_read_only;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use codex_router_state::sqlite::StateStoreError;
use opentelemetry::KeyValue;
use opentelemetry::global;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;
use crate::credential_runtime::AsyncProviderCredentialResolver;
use crate::credential_runtime::CliCredentialResolver;
use crate::credential_runtime::CliCredentialResolverOpenError;
use crate::presentation::quota::QuotaSelectedAccountViewModel;
use crate::presentation::quota::QuotaStatusAccountViewModel;
use crate::presentation::quota::QuotaStatusViewModel;
use crate::presentation::quota::QuotaStatusViewModelLoader;
use crate::presentation::quota::ResetPaceMeterSegments;
use crate::presentation::quota::ResetPaceState;
use crate::presentation::quota::ResetPaceViewModel;
use crate::presentation::quota::SampleConfidence;
use crate::presentation::quota::SampleMetadata;
use crate::presentation::quota::WeeklyQuotaFloorSaveError;
use crate::presentation::quota::WeeklyQuotaFloorSaver;
use crate::presentation::quota::run_quota_status_view;
use crate::presentation::quota::write_quota_status_view;
use crate::router_root_or_default;

const DEFAULT_ROUTE_BANDS: &[&str] = &["responses", "models"];
const USER_QUOTA_ROUTE_BAND: &str = "responses";
pub(crate) const DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS: u64 = 360;
const QUOTA_STATUS_SAMPLE_FRESH_SECONDS: u64 = 900;
const QUOTA_STATUS_SHORT_BURN_LOOKBACK_SECONDS: u64 = 30 * 60;
const QUOTA_STATUS_WEEKLY_BURN_LOOKBACK_SECONDS: u64 = 3 * 60 * 60;
const QUOTA_STATUS_DISPLAY_MIN_RATE_SAMPLES: usize = 3;
const QUOTA_STATUS_DISPLAY_NORMAL_CONFIDENCE_SAMPLES: usize = 5;
const RESET_PACE_RUNOUT_LABEL_THRESHOLD_HUNDREDTHS: u32 = 200;
const ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS: u64 = 7_200;
const DEPLETED_QUOTA_LABEL: &str = "Exhausted";

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

mod quota_background_refresh_worker;
mod quota_command_dispatch;
mod quota_refresh_command;
mod quota_refresh_history;
mod quota_refresh_provider;
mod quota_refresh_service;

pub(crate) use quota_background_refresh_worker::*;
pub use quota_command_dispatch::*;
pub(crate) use quota_refresh_command::*;
use quota_refresh_history::*;
pub(crate) use quota_refresh_provider::*;
pub(crate) use quota_refresh_service::*;

mod quota_command_options;
mod quota_reset_pace_projection;
mod quota_route_selection_projection;
mod quota_status_command;
mod quota_status_formatting;
mod quota_status_json;
mod quota_status_loader;
mod quota_status_metrics;
mod quota_status_projection;
mod quota_status_view_model;

use quota_command_options::*;
use quota_reset_pace_projection::*;
use quota_route_selection_projection::*;
use quota_status_command::*;
use quota_status_formatting::*;
use quota_status_json::*;
use quota_status_loader::*;
use quota_status_metrics::*;
use quota_status_projection::*;
use quota_status_view_model::*;

#[cfg(test)]
mod quota_command_family_test;
