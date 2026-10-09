use super::assessment_selection::{
    select_from_account_states_with_selector, select_from_burn_down_assessment_without_hold,
};
use super::post_exhaustion::post_exhaustion_assessment_has_safe_known_fresh_alternative;
use super::short_quota_wait::bounded_positive_test_jitter;
use super::*;

use super::AccountDecisionSelector;
use super::ActiveClientLeaseReporter;
use super::ActiveReservationGuard;
use super::AsyncAccountDecisionSelector;
use super::QuotaAwareAccountSelector;
use super::QuotaAwareAccountState;
use super::RouteBandReservationBooks;
use super::SqliteActiveClientLeaseReporter;
use crate::routes::RouteKind;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::CLAUDE_MESSAGES;
use codex_router_core::route_profile::RESPONSES_HTTP;
use codex_router_core::routes::RouteBand;
use codex_router_quota::snapshot::SnapshotFreshness;
use codex_router_selection::reservation::ReservationBook;
use codex_router_selection::reservation::ReservationHandle;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::db_write_actor::DbWriteActor;
use crate::db_write_actor::DbWriteRepository;
use crate::db_write_actor::DbWriteRepositoryError;
use crate::db_write_actor::SqliteDbWriteRepository;
use crate::provider_error::ProviderErrorClassification;
use crate::session_account_affinity_cache::DEFAULT_SESSION_PIN_IDLE_TTL;
use crate::test_log_capture::capture_log_output;

#[path = "account_selection_tests/lease_report_fixtures.rs"]
mod lease_report_fixtures;
use lease_report_fixtures::*;
#[path = "account_selection_tests/projection_read_fixtures.rs"]
mod projection_read_fixtures;
use projection_read_fixtures::*;
#[path = "account_selection_tests/affinity_race_fixtures.rs"]
mod affinity_race_fixtures;
use affinity_race_fixtures::*;
#[path = "account_selection_tests/affinity_write_fixtures.rs"]
mod affinity_write_fixtures;
use affinity_write_fixtures::*;
#[path = "account_selection_tests/quota_input_fixtures.rs"]
mod quota_input_fixtures;
use quota_input_fixtures::*;
#[path = "account_selection_tests/test_identity_fixtures.rs"]
mod test_identity_fixtures;
use test_identity_fixtures::*;
#[path = "account_selection_tests/lease_wait_fixtures.rs"]
mod lease_wait_fixtures;
use lease_wait_fixtures::*;
#[path = "account_selection_tests/affinity_reconciliation_tests.rs"]
mod affinity_reconciliation_tests;
#[path = "account_selection_tests/claude_admission_tests.rs"]
mod claude_admission_tests;
#[path = "account_selection_tests/last_resort_tests.rs"]
mod last_resort_tests;
#[path = "account_selection_tests/post_exhaustion_tests.rs"]
mod post_exhaustion_tests;
#[path = "account_selection_tests/provider_hold_tests.rs"]
mod provider_hold_tests;
#[path = "account_selection_tests/reservation_lease_tests.rs"]
mod reservation_lease_tests;
#[path = "account_selection_tests/selection_authority_tests.rs"]
mod selection_authority_tests;
#[path = "account_selection_tests/selection_concurrency_tests.rs"]
mod selection_concurrency_tests;
#[path = "account_selection_tests/selection_diagnostics_tests.rs"]
mod selection_diagnostics_tests;
#[path = "account_selection_tests/short_quota_tests.rs"]
mod short_quota_tests;
