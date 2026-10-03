use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;
use codex_router_core::ids::ReservationId;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use codex_router_selection::run_rate::QuotaRunRateConfidence;
use rusqlite::Connection;
use sqlx::Row;

use super::package_name;
use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::account_routing_policy::AccountRoutingPolicy;
use crate::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use crate::affinity_owner::AffinitySourceTransport;
use crate::affinity_owner::PreviousResponseAffinityOwnerLookup;
use crate::affinity_owner::PreviousResponseAffinityOwnerRecord;
use crate::credential_maintenance::ClaimPurpose;
use crate::credential_maintenance::CredentialMaintenanceState;
use crate::credential_maintenance::CredentialRefreshClaimDisposition;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::quota_snapshot::QuotaHistoryRefreshOutcome;
use crate::quota_snapshot::QuotaRefreshErrorClass;
use crate::quota_snapshot::QuotaRefreshStatusSource;
use crate::quota_snapshot::QuotaSnapshotSource;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
use crate::repositories::AccountStateRepository;
use crate::repositories::AffinityRepository;
use crate::repositories::QuotaSnapshotRepository;
use crate::repositories::SelectorQuotaRepository;
use crate::selection_projection::project_route_band_selection_inputs;
use crate::selection_projection::project_route_band_selection_inputs_with_active_counts;
use crate::session_account_affinity::PinObservation;
use crate::session_account_affinity::SessionAccountAffinity;
use crate::sqlite::AsyncAffinityRepository;
use crate::sqlite::AsyncQuotaExhaustionRepository;
use crate::sqlite::AsyncQuotaHistoryRepository;
use crate::sqlite::AsyncSelectorQuotaRepository;
use crate::sqlite::AsyncSessionAccountAffinityRepository;
use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use crate::sqlite::SqliteStateStore;
use crate::sqlite::StateStoreError;
use crate::sqlite::WeeklyQuotaFloorMutationResult;

#[path = "tests/credential_maintenance_store_tests.rs"]
mod credential_maintenance_store_tests;

#[path = "state_tests/test_identity_fixtures.rs"]
mod test_identity_fixtures;

#[path = "state_tests/policy_schema_fixtures.rs"]
mod policy_schema_fixtures;

#[path = "state_tests/quota_schema_fixtures.rs"]
mod quota_schema_fixtures;

#[path = "state_tests/session_schema_fixtures.rs"]
mod session_schema_fixtures;

use test_identity_fixtures::{
    TestTempDir, account_id, affinity_hash, assert_no_previous_response_id_in_affinity_owner_rows,
    expect_error, quota_history_observation,
};

use policy_schema_fixtures::{
    assert_credit_migration_tables, assert_intact_v11_policy_database,
    convert_current_fixture_to_v10, convert_current_fixture_to_v11, convert_current_fixture_to_v12,
};

use quota_schema_fixtures::{
    create_v2_database_with_quota_snapshot,
    create_v3_database_with_legacy_code_review_selector_window,
    create_v6_database_without_reset_credits,
};

use session_schema_fixtures::{
    create_partial_v10_database_missing_active_session_columns,
    create_v8_database_with_current_active_lease,
    create_v10_database_missing_async_projection_tables,
};

#[path = "state_tests/credential_claim_tests.rs"]
mod credential_claim_tests;

#[path = "state_tests/session_affinity_tests.rs"]
mod session_affinity_tests;

#[path = "state_tests/provider_schema_tests.rs"]
mod provider_schema_tests;

#[path = "state_tests/floor_migration_tests.rs"]
mod floor_migration_tests;

#[path = "state_tests/floor_schema_tests.rs"]
mod floor_schema_tests;

#[path = "state_tests/floor_mutation_tests.rs"]
mod floor_mutation_tests;

#[path = "state_tests/storage_boundary_tests.rs"]
mod storage_boundary_tests;

#[path = "state_tests/store_projection_tests.rs"]
mod store_projection_tests;

#[path = "state_tests/read_only_tests.rs"]
mod read_only_tests;

#[path = "state_tests/quota_history_tests.rs"]
mod quota_history_tests;

#[path = "state_tests/active_lease_tests.rs"]
mod active_lease_tests;

#[path = "state_tests/session_history_tests.rs"]
mod session_history_tests;

#[path = "state_tests/session_terminal_tests.rs"]
mod session_terminal_tests;

#[path = "state_tests/burn_projection_tests.rs"]
mod burn_projection_tests;

#[path = "state_tests/quota_exhaustion_tests.rs"]
mod quota_exhaustion_tests;

#[path = "state_tests/quota_refresh_tests.rs"]
mod quota_refresh_tests;

#[path = "state_tests/credential_mutation_tests.rs"]
mod credential_mutation_tests;

#[path = "state_tests/repository_contract_tests.rs"]
mod repository_contract_tests;

#[path = "state_tests/response_affinity_tests.rs"]
mod response_affinity_tests;

#[path = "state_tests/account_order_tests.rs"]
mod account_order_tests;
