use super::*;

use super::credit_turn_test_support::CREDIT_TURN_FIXTURE_TIME;
use super::credit_turn_test_support::CreditTurnFixture;
use super::credit_turn_test_support::CreditTurnTestDirectory;
use crate::account_selection::AccountSourceAdmission;
use crate::account_selection::mark_runtime_quota_exhausted;
use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::credit_usage::CreditSpendControl;
use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::account_routing_policy::WeeklyQuotaFloorBasisPoints;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::sqlite::AsyncWeeklyQuotaFloorMutationStore;
use codex_router_state::sqlite::SqliteStateStore;

#[tokio::test]
async fn source_assessment_rejects_provider_credit_depletion_and_spend_controls() {
    let positive_credit = CreditAvailability::Available {
        balance: Some(CreditBalance::new("2.75").expect("credit balance should validate")),
    };
    let scenarios = [
        (
            "depleted entitlement",
            CreditAvailability::Depleted,
            CreditSpendControl::Clear,
            Some(CreditProviderLimitReason::RateLimitReached),
        ),
        (
            "unknown entitlement",
            CreditAvailability::Unknown,
            CreditSpendControl::Unreported,
            None,
        ),
        (
            "reached provider spend control",
            positive_credit.clone(),
            CreditSpendControl::Reached,
            Some(CreditProviderLimitReason::RateLimitReached),
        ),
        (
            "unknown provider spend control",
            positive_credit.clone(),
            CreditSpendControl::Unknown,
            Some(CreditProviderLimitReason::RateLimitReached),
        ),
        (
            "workspace credit rejection",
            positive_credit.clone(),
            CreditSpendControl::Clear,
            Some(CreditProviderLimitReason::WorkspaceOwnerCreditsDepleted),
        ),
        (
            "unknown provider rejection",
            positive_credit,
            CreditSpendControl::Clear,
            Some(CreditProviderLimitReason::Unknown),
        ),
    ];

    for (label, availability, spend_control, limit_reason) in scenarios {
        let directory = CreditTurnTestDirectory::new();
        let fixture = CreditTurnFixture::new(&directory).await;
        let provider_observation =
            CreditProviderObservation::new(availability, spend_control, limit_reason);
        fixture
            .replace_provider_observation(&provider_observation)
            .await;

        assert_eq!(
            fixture
                .assessor
                .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
                .await,
            AccountSourceAdmission::ReconnectRequired,
            "source account must not continue after {label}"
        );
        close_fixture(&fixture).await;
    }
}

#[tokio::test]
async fn source_assessment_rejects_pending_or_replaced_credit_generations() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::PermittedByCreditBackedQuota
    );
    assert_eq!(
        fixture
            .assessor_at(CREDIT_TURN_FIXTURE_TIME + 301)
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "expired credit observation must not extend an existing session"
    );
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 2, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "pinned generation mismatch must fail closed"
    );

    fixture
        .writer
        .begin_credit_refresh_attempt(&fixture.account_id, 1)
        .await
        .expect("newer provider read should mark older credit as pending");
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "a pending latest attempt must suppress the previous observation"
    );

    let synchronous_state = SqliteStateStore::open(&fixture.database_path)
        .expect("generation replacement state should open");
    let replaced_account = AccountRecord::new(
        Provider::Openai,
        fixture.account_id.clone(),
        "credit-turn-source",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(2);
    AccountStateRepository::upsert_account(&synchronous_state, &replaced_account)
        .expect("credential generation should rotate");
    drop(synchronous_state);
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "source socket must not continue on a replaced credential generation"
    );
    close_fixture(&fixture).await;
}

#[tokio::test]
async fn source_assessment_preserves_included_quota_and_existing_runtime_guards() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    fixture.commit_included_quota().await;
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::Permitted,
        "healthy included quota should allow the current source without credit authority"
    );

    fixture
        .writer
        .save_account_credit_usage_policy(&fixture.account_id, CreditUsagePolicy::Allow)
        .await
        .expect("credit opt-in should be restored");
    fixture
        .replace_provider_observation(&CreditProviderObservation::new(
            CreditAvailability::Available {
                balance: Some(CreditBalance::new("2.75").expect("valid credit balance")),
            },
            CreditSpendControl::Clear,
            Some(CreditProviderLimitReason::RateLimitReached),
        ))
        .await;
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::PermittedByCreditBackedQuota,
        "fresh positive credits should be eligible before the floor is configured"
    );

    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&fixture.database_path)
        .await
        .expect("floor mutation store should open");
    mutation
        .set_weekly_quota_floor_by_account_id(
            &fixture.account_id,
            Some(WeeklyQuotaFloorBasisPoints::new(100).expect("valid floor")),
        )
        .await
        .expect("hard floor should persist");
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "a configured hard floor must keep its existing priority"
    );
    mutation
        .set_weekly_quota_floor_by_account_id(&fixture.account_id, None)
        .await
        .expect("hard floor should clear");

    mark_runtime_quota_exhausted(
        &fixture.runtime_exhaustions,
        RouteBand::Responses,
        fixture.account_id.clone(),
        CREDIT_TURN_FIXTURE_TIME,
    )
    .expect("runtime quota rejection should be recorded");
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "existing runtime rejection must block source continuation"
    );
    fixture
        .runtime_exhaustions
        .lock()
        .expect("runtime exhaustion map should remain readable")
        .clear();

    mutation
        .set_account_status_by_label("credit-turn-source", AccountStatus::Disabled)
        .await
        .expect("account should disable");
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "disabled account must not continue on an existing socket"
    );

    mutation
        .set_account_status_by_label("credit-turn-source", AccountStatus::Enabled)
        .await
        .expect("account should re-enable");
    let synchronous_state =
        SqliteStateStore::open(&fixture.database_path).expect("credential state should open");
    let credentialless_account = AccountRecord::new(
        Provider::Openai,
        fixture.account_id.clone(),
        "credit-turn-source",
        AccountStatus::Enabled,
    );
    AccountStateRepository::upsert_account(&synchronous_state, &credentialless_account)
        .expect("credential should be removed");
    drop(synchronous_state);
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired,
        "missing credentials must not continue on an existing socket"
    );

    close_fixture(&fixture).await;
}

async fn close_fixture(fixture: &CreditTurnFixture) {
    fixture
        .writer
        .close()
        .await
        .expect("credit-turn writer should close");
    fixture
        .reader
        .close()
        .await
        .expect("credit-turn reader should close");
}
