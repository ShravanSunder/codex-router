use codex_router_core::ids::AccountId;

use crate::quota_reset::reset_credit_policy::ActiveCredentialGeneration;
use crate::quota_reset::reset_credit_policy::AttemptGeneration;
use crate::quota_reset::reset_credit_policy::ConsumePortResult;
use crate::quota_reset::reset_credit_policy::ConsumeUnknownReason;
use crate::quota_reset::reset_credit_policy::CreditInventoryPortResult;
use crate::quota_reset::reset_credit_policy::KnownConsumeOutcome;
use crate::quota_reset::reset_credit_policy::LiveResetCredit;
use crate::quota_reset::reset_credit_policy::LiveUsagePortResult;
use crate::quota_reset::reset_credit_policy::LiveWeeklyUsage;
use crate::quota_reset::reset_credit_policy::OperationGeneration;
use crate::quota_reset::reset_credit_policy::RenderSafeFailure;
use crate::quota_reset::reset_credit_policy::validate_credit_inventory;

use super::correlated_effect_contracts::*;
use super::reset_workflow_state::*;

#[test]
fn requests_preserve_correlation_identity() {
    let correlation = OperationCorrelation::new(
        AccountId::new("account-a")
            .unwrap_or_else(|error| panic!("account id should be valid: {error}")),
        ActiveCredentialGeneration::new(7),
        AttemptGeneration::new(11),
        OperationGeneration::new(13),
    );
    let requests = [
        CorrelatedRequest::inspection_live_usage(correlation.clone()),
        CorrelatedRequest::inspection_credit_inventory(correlation.clone()),
        CorrelatedRequest::revalidation_live_usage(correlation.clone()),
        CorrelatedRequest::revalidation_credit_inventory(correlation.clone()),
        CorrelatedRequest::consume_credit(correlation.clone()),
    ];
    for request in requests {
        assert_eq!(request.correlation(), &correlation);
    }
    assert_eq!(
        correlation.active_credential_generation(),
        ActiveCredentialGeneration::new(7)
    );
    assert_eq!(correlation.attempt_generation(), AttemptGeneration::new(11));
}

#[test]
fn activity_and_consume_results_have_only_render_safe_categories() {
    let previous = LiveWeeklyUsage::new(0);
    let activities = [
        OperationActivity::NotStarted,
        OperationActivity::Loading,
        OperationActivity::Refreshing {
            previous: Some(previous),
        },
        OperationActivity::Succeeded(previous),
        OperationActivity::Failed {
            failure: RenderSafeFailure::Transport,
            previous: Some(previous),
        },
        OperationActivity::Cancelled,
        OperationActivity::RequestDispatchedAwaitingOutcome,
    ];

    assert_eq!(activities.len(), 7);
    assert_eq!(WorkflowPhase::default(), WorkflowPhase::Browse);
    assert_ne!(
        RenderValueProvenance::PreviousLiveRefreshing,
        RenderValueProvenance::CurrentLive
    );
    assert_eq!(eligible_inventory_result(), eligible_inventory_result());
    assert_eq!(
        ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed),
        ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed)
    );
    assert!(matches!(
        ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::Transport),
        ConsumePortResult::OutcomeUnknown(_)
    ));
    assert_eq!(
        RenderSafeFailure::TimedOut.message(),
        "provider operation timed out"
    );
    let source = include_str!("correlated_effect_contracts.rs");
    assert!(!source.contains(&["RenderSafeFailure", "::new"].concat()));
    let domain_source = include_str!("../reset_credit_policy.rs");
    let failure_contract = domain_source
        .split("pub(crate) enum RenderSafeFailure")
        .nth(1)
        .and_then(|source| source.split("/// Validated live weekly usage").next())
        .unwrap_or_else(|| panic!("render-safe failure contract should exist"));
    assert!(!failure_contract.contains("String"));
    assert!(!failure_contract.contains("fn new"));
    let redeem_contract = domain_source
        .split("struct RedeemRequestId")
        .nth(1)
        .and_then(|source| source.split("/// The five provider operations").next())
        .unwrap_or_else(|| panic!("redeem request identity contract should exist"));
    assert!(redeem_contract.contains("fn new"));
    assert!(redeem_contract.contains("as_str"));
}

#[test]
fn commit_capability_source_contract_is_opaque_and_by_value() {
    let source = include_str!("../reset_commit_service.rs");
    let capability_start = source
        .find("pub(in crate::quota_reset) struct CommitCapability")
        .unwrap_or_else(|| panic!("commit capability declaration should exist"));
    let capability_source = source
        .get(capability_start..)
        .unwrap_or_else(|| panic!("capability source range should exist"));
    let declaration_prefix = source
        .get(..capability_start)
        .unwrap_or_else(|| panic!("capability declaration prefix should exist"));
    let preceding_declaration = declaration_prefix
        .get(declaration_prefix.len().saturating_sub(160)..)
        .unwrap_or(declaration_prefix);
    let capability_body = capability_source
        .split("impl<TAuthority, TPreparedConsume> std::fmt::Debug")
        .next()
        .unwrap_or(capability_source);

    assert!(!preceding_declaration.contains(&["derive", "(Clone"].concat()));
    assert!(!preceding_declaration.contains("Serialize"));
    assert!(!capability_source.contains(&["impl Clone", " for CommitCapability"].concat()));
    assert!(!capability_source.contains(&["Serialize", " for CommitCapability"].concat()));
    assert!(!capability_body.contains("pub(crate)"));
    assert!(source.contains("pub(in crate::quota_reset) struct CommitCapability"));
    assert!(source.contains("capability: CommitCapability"));
    assert!(!source.contains(&["capability: ", "&CommitCapability"].concat()));
}

#[test]
fn reducer_happy_path_requires_both_inspection_and_revalidation_results() {
    let mut workflow = ResetWorkflow::default();
    let inspection = inspection_start("account-a", 1, 10, 100, 101);
    let effects = workflow.reduce(WorkflowIntent::BeginInspection(inspection.clone()));
    assert_eq!(workflow.phase(), WorkflowPhase::Inspecting);
    assert_eq!(effects.len(), 2);

    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_live_usage(
            inspection.live_usage_correlation(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
    ));
    assert_eq!(workflow.phase(), WorkflowPhase::Inspecting);
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_credit_inventory(
            inspection.credit_inventory_correlation(),
            eligible_inventory_result(),
        ),
    ));
    assert_eq!(workflow.phase(), WorkflowPhase::Inspected);

    workflow.reduce(WorkflowIntent::OpenConfirmation);
    assert_eq!(workflow.confirmation_selection(), ConfirmationSelection::No);
    assert!(workflow.yes_enabled());
    workflow.reduce(WorkflowIntent::SelectYes);
    let effects = workflow.reduce(WorkflowIntent::Confirm {
        live_usage_operation_generation: OperationGeneration::new(200),
        credit_inventory_operation_generation: OperationGeneration::new(201),
    });
    assert_eq!(workflow.phase(), WorkflowPhase::Revalidating);
    assert_eq!(effects.len(), 2);
}

#[test]
fn reducer_disables_yes_and_suppresses_repeated_or_stale_completions() {
    let mut workflow = ResetWorkflow::default();
    let inspection = inspection_start("account-a", 1, 10, 100, 101);
    assert_eq!(
        workflow
            .reduce(WorkflowIntent::BeginInspection(inspection.clone()))
            .len(),
        2
    );
    assert!(
        workflow
            .reduce(WorkflowIntent::BeginInspection(inspection.clone()))
            .is_empty()
    );

    let stale = inspection_start("account-a", 1, 9, 90, 91);
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_live_usage(
            stale.live_usage_correlation(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
    ));
    assert_eq!(workflow.phase(), WorkflowPhase::Inspecting);
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_live_usage(
            inspection.live_usage_correlation(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(10)),
        ),
    ));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_credit_inventory(
            inspection.credit_inventory_correlation(),
            inventory_result_with_expiry(43_301),
        ),
    ));
    workflow.reduce(WorkflowIntent::OpenConfirmation);
    assert!(!workflow.yes_enabled());
    workflow.reduce(WorkflowIntent::SelectYes);
    assert_eq!(workflow.confirmation_selection(), ConfirmationSelection::No);
}

#[test]
fn yes_enabled_allows_below_ten_percent_or_imminent_credit_expiry() {
    for (remaining_percent, credit_expiration, expected_enabled) in
        [(9, 50_000, true), (10, 43_300, true), (10, 43_301, false)]
    {
        let mut workflow = ResetWorkflow::default();
        let inspection = inspection_start("account-a", 1, 10, 100, 101);
        workflow.reduce(WorkflowIntent::BeginInspection(inspection.clone()));
        workflow.reduce(WorkflowIntent::OperationCompleted(
            CorrelatedOutcome::inspection_live_usage(
                inspection.live_usage_correlation(),
                LiveUsagePortResult::Known(LiveWeeklyUsage::new(remaining_percent)),
            ),
        ));
        workflow.reduce(WorkflowIntent::OperationCompleted(
            CorrelatedOutcome::inspection_credit_inventory(
                inspection.credit_inventory_correlation(),
                inventory_result_with_expiry(credit_expiration),
            ),
        ));
        workflow.reduce(WorkflowIntent::OpenConfirmation);

        assert_eq!(
            workflow.yes_enabled(),
            expected_enabled,
            "remaining={remaining_percent}, expiration={credit_expiration}"
        );
    }
}

#[test]
fn previous_and_current_live_provenance_never_share_authority() {
    let mut workflow = ResetWorkflow::default();
    let first = inspection_start("account-a", 1, 10, 100, 101);
    workflow.reduce(WorkflowIntent::BeginInspection(first.clone()));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_live_usage(
            first.live_usage_correlation(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
    ));
    workflow.reduce(WorkflowIntent::Cancel);
    assert_eq!(
        workflow.live_usage.as_ref().map(|usage| usage.provenance),
        Some(RenderValueProvenance::CurrentLive)
    );

    workflow.reduce(WorkflowIntent::BeginInspection(inspection_start(
        "account-a",
        1,
        11,
        110,
        111,
    )));
    assert_eq!(
        workflow.live_usage.as_ref().map(|usage| usage.provenance),
        Some(RenderValueProvenance::PreviousLiveRefreshing)
    );
    assert!(!workflow.yes_enabled());
}

#[test]
fn new_inspection_failure_keeps_both_previous_observations_without_authority() {
    let mut workflow = ResetWorkflow::default();
    let first = inspection_start("account-a", 1, 10, 100, 101);
    workflow.reduce(WorkflowIntent::BeginInspection(first.clone()));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_live_usage(
            first.live_usage_correlation(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
    ));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_credit_inventory(
            first.credit_inventory_correlation(),
            eligible_inventory_result(),
        ),
    ));
    assert!(workflow.yes_enabled());
    workflow.reduce(WorkflowIntent::Cancel);

    let second = inspection_start("account-a", 1, 11, 110, 111);
    workflow.reduce(WorkflowIntent::BeginInspection(second.clone()));
    assert_eq!(
        workflow
            .live_usage_observation()
            .map(|(_, provenance)| provenance),
        Some(RenderValueProvenance::PreviousLiveRefreshing)
    );
    assert_eq!(
        workflow
            .inventory_observation()
            .map(|(_, provenance)| provenance),
        Some(RenderValueProvenance::PreviousLiveRefreshing)
    );

    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_live_usage(
            second.live_usage_correlation(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
    ));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_credit_inventory(
            second.credit_inventory_correlation(),
            CreditInventoryPortResult::Failed(RenderSafeFailure::Transport),
        ),
    ));

    assert_eq!(workflow.phase(), WorkflowPhase::Inspected);
    assert_eq!(
        workflow
            .inventory_observation()
            .map(|(_, provenance)| provenance),
        Some(RenderValueProvenance::PreviousLiveRefreshing)
    );
    assert!(!workflow.yes_enabled());
}

#[test]
fn yes_enabled_requires_current_successful_operation_state() {
    let mut workflow = eligible_confirming_workflow();
    assert_eq!(
        workflow
            .inventory_observation()
            .map(|(_, provenance)| provenance),
        Some(RenderValueProvenance::CurrentLive)
    );
    workflow.activities.inspection_credit_inventory = OperationActivity::Failed {
        failure: RenderSafeFailure::Transport,
        previous: None,
    };

    assert!(!workflow.yes_enabled());
}

#[test]
fn authority_loss_returns_confirmation_to_no_and_consume_is_conservative() {
    let mut workflow = eligible_confirming_workflow();
    workflow.reduce(WorkflowIntent::SelectYes);
    assert_eq!(
        workflow.confirmation_selection(),
        ConfirmationSelection::Yes
    );
    workflow.reduce(WorkflowIntent::AuthorityLost(
        RenderSafeFailure::CredentialGenerationChanged,
    ));
    assert_eq!(workflow.confirmation_selection(), ConfirmationSelection::No);
    assert!(!workflow.yes_enabled());

    let mut committing = committing_workflow();
    committing.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::consume_credit(
            committing
                .consume_correlation()
                .unwrap_or_else(|| panic!("consume correlation")),
            ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::TimedOut),
        ),
    ));
    assert_eq!(
        committing.result(),
        Some(&WorkflowResult::OutcomeUnknown(
            ConsumeUnknownReason::TimedOut
        ))
    );
}

#[test]
fn correlated_revalidation_refusal_enters_dismissible_result() {
    let mut workflow = eligible_confirming_workflow();
    workflow.reduce(WorkflowIntent::SelectYes);
    workflow.reduce(WorkflowIntent::Confirm {
        live_usage_operation_generation: OperationGeneration::new(200),
        credit_inventory_operation_generation: OperationGeneration::new(201),
    });
    let usage_correlation = workflow
        .revalidation_usage
        .clone()
        .unwrap_or_else(|| panic!("revalidation usage correlation"));
    let inventory_correlation = workflow
        .revalidation_inventory
        .clone()
        .unwrap_or_else(|| panic!("revalidation inventory correlation"));
    let stale_usage_correlation =
        inspection_start("account-a", 1, 9, 90, 91).live_usage_correlation();

    workflow.reduce(WorkflowIntent::RevalidationRefused {
        live_usage_correlation: stale_usage_correlation,
        credit_inventory_correlation: inventory_correlation.clone(),
        failure: RenderSafeFailure::EligibilityRefused,
    });
    assert_eq!(workflow.phase(), WorkflowPhase::Revalidating);
    assert_eq!(workflow.result(), None);

    workflow.reduce(WorkflowIntent::RevalidationRefused {
        live_usage_correlation: usage_correlation,
        credit_inventory_correlation: inventory_correlation,
        failure: RenderSafeFailure::EligibilityRefused,
    });
    assert_eq!(workflow.phase(), WorkflowPhase::Result);
    assert_eq!(
        workflow.result(),
        Some(&WorkflowResult::Refused(
            RenderSafeFailure::EligibilityRefused
        ))
    );

    workflow.reduce(WorkflowIntent::Cancel);
    assert_eq!(workflow.phase(), WorkflowPhase::Browse);
    assert_eq!(workflow.result(), None);
}

fn inspection_start(
    account: &str,
    credential_generation: u64,
    attempt_generation: u64,
    usage_operation: u64,
    credit_operation: u64,
) -> InspectionStart {
    InspectionStart::new(
        AccountId::new(account)
            .unwrap_or_else(|error| panic!("account id should be valid: {error}")),
        ActiveCredentialGeneration::new(credential_generation),
        AttemptGeneration::new(attempt_generation),
        OperationGeneration::new(usage_operation),
        OperationGeneration::new(credit_operation),
    )
}

fn eligible_inventory_result() -> CreditInventoryPortResult {
    inventory_result_with_expiry(200)
}

fn inventory_result_with_expiry(expires_unix_seconds: i64) -> CreditInventoryPortResult {
    CreditInventoryPortResult::Validated(
        validate_credit_inventory(
            vec![LiveResetCredit {
                id: "credit-a".to_owned(),
                status: "available".to_owned(),
                expires_unix_seconds: Some(expires_unix_seconds),
                expires_at: Some(format!("unix-{expires_unix_seconds}")),
                title: Some("Weekly reset".to_owned()),
            }],
            100,
        )
        .unwrap_or_else(|error| panic!("inventory should validate: {error:?}")),
    )
}

fn eligible_confirming_workflow() -> ResetWorkflow {
    let mut workflow = ResetWorkflow::default();
    let inspection = inspection_start("account-a", 1, 10, 100, 101);
    workflow.reduce(WorkflowIntent::BeginInspection(inspection.clone()));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_live_usage(
            inspection.live_usage_correlation(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
    ));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_credit_inventory(
            inspection.credit_inventory_correlation(),
            eligible_inventory_result(),
        ),
    ));
    workflow.reduce(WorkflowIntent::OpenConfirmation);
    workflow
}

fn committing_workflow() -> ResetWorkflow {
    let mut workflow = eligible_confirming_workflow();
    workflow.reduce(WorkflowIntent::SelectYes);
    workflow.reduce(WorkflowIntent::Confirm {
        live_usage_operation_generation: OperationGeneration::new(200),
        credit_inventory_operation_generation: OperationGeneration::new(201),
    });
    let usage_correlation = workflow
        .revalidation_usage
        .clone()
        .unwrap_or_else(|| panic!("revalidation usage correlation"));
    let inventory_correlation = workflow
        .revalidation_inventory
        .clone()
        .unwrap_or_else(|| panic!("revalidation inventory correlation"));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::revalidation_live_usage(
            usage_correlation,
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
    ));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::revalidation_credit_inventory(
            inventory_correlation,
            eligible_inventory_result(),
        ),
    ));
    let effects = workflow.reduce(WorkflowIntent::CommitAuthorized {
        consume_operation_generation: OperationGeneration::new(300),
    });
    assert_eq!(effects.len(), 1);
    assert_eq!(workflow.phase(), WorkflowPhase::Committing);
    workflow
}
