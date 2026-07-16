use codex_router_core::ids::AccountId;

use crate::quota_reset::domain::ActiveCredentialGeneration;
use crate::quota_reset::domain::AttemptGeneration;
use crate::quota_reset::domain::ConsumePortResult;
use crate::quota_reset::domain::ConsumeUnknownReason;
use crate::quota_reset::domain::CreditInventoryPortResult;
use crate::quota_reset::domain::KnownConsumeOutcome;
use crate::quota_reset::domain::LiveResetCredit;
use crate::quota_reset::domain::LiveUsagePortResult;
use crate::quota_reset::domain::LiveWeeklyUsage;
use crate::quota_reset::domain::OperationGeneration;
use crate::quota_reset::domain::OperationKind;
use crate::quota_reset::domain::RenderSafeFailure;
use crate::quota_reset::domain::validate_credit_inventory;

use super::contracts::*;
use super::model::*;

#[test]
fn operation_kinds_are_the_five_visible_provider_operations() {
    assert_eq!(
        OperationKind::ALL,
        [
            OperationKind::InspectionLiveUsage,
            OperationKind::InspectionCreditInventory,
            OperationKind::RevalidationLiveUsage,
            OperationKind::RevalidationCreditInventory,
            OperationKind::ConsumeCredit,
        ]
    );
}

#[test]
fn request_and_outcome_repeat_all_correlation_fields() {
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
    let outcomes = [
        CorrelatedOutcome::inspection_live_usage(
            correlation.clone(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
        CorrelatedOutcome::inspection_credit_inventory(
            correlation.clone(),
            eligible_inventory_result(),
        ),
        CorrelatedOutcome::revalidation_live_usage(
            correlation.clone(),
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
        ),
        CorrelatedOutcome::revalidation_credit_inventory(
            correlation.clone(),
            eligible_inventory_result(),
        ),
        CorrelatedOutcome::consume_credit(
            correlation.clone(),
            ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed),
        ),
    ];

    for ((request, outcome), expected_kind) in
        requests.iter().zip(outcomes.iter()).zip(OperationKind::ALL)
    {
        assert_eq!(request.operation_kind(), expected_kind);
        assert_eq!(outcome.operation_kind(), expected_kind);
        assert_eq!(request.correlation(), &correlation);
        assert_eq!(outcome.correlation(), &correlation);
    }
    assert_eq!(correlation.active_credential_generation().get(), 7);
    assert_eq!(correlation.attempt_generation().get(), 11);
    assert_eq!(correlation.operation_generation().get(), 13);
    assert_eq!(
        outcomes[0].live_usage_terminal(),
        Some(&LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)))
    );
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
        RenderValueProvenance::Saved,
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
    let source = include_str!("contracts.rs");
    assert!(!source.contains(&["RenderSafeFailure", "::new"].concat()));
    let domain_source = include_str!("../domain.rs");
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
    let source = include_str!("../service.rs");
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
            LiveUsagePortResult::Known(LiveWeeklyUsage::new(1)),
        ),
    ));
    workflow.reduce(WorkflowIntent::OperationCompleted(
        CorrelatedOutcome::inspection_credit_inventory(
            inspection.credit_inventory_correlation(),
            eligible_inventory_result(),
        ),
    ));
    workflow.reduce(WorkflowIntent::OpenConfirmation);
    assert!(!workflow.yes_enabled());
    workflow.reduce(WorkflowIntent::SelectYes);
    assert_eq!(workflow.confirmation_selection(), ConfirmationSelection::No);
}

#[test]
fn saved_previous_and_current_provenance_never_share_authority() {
    let mut workflow = ResetWorkflow::default();
    workflow.set_saved_usage(LiveWeeklyUsage::new(0));
    assert_eq!(
        workflow.live_usage.as_ref().map(|usage| usage.provenance),
        Some(RenderValueProvenance::Saved)
    );
    assert!(!workflow.yes_enabled());

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
    CreditInventoryPortResult::Validated(
        validate_credit_inventory(
            vec![LiveResetCredit {
                id: "credit-a".to_owned(),
                status: "available".to_owned(),
                expires_unix_seconds: Some(200),
                expires_at: Some("unix-200".to_owned()),
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
