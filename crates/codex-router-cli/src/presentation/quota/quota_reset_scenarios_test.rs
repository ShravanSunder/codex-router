fn reset_operation_scenarios() -> Vec<(&'static str, ResetWorkflowSnapshot, &'static [&'static str])>
{
    let inspecting = WorkflowActivities {
        inspection_live_usage: OperationActivity::Loading,
        inspection_credit_inventory: OperationActivity::Loading,
        ..WorkflowActivities::default()
    };

    let inspection_partial = WorkflowActivities {
        inspection_live_usage: OperationActivity::Succeeded(
            crate::quota_reset::reset_session_supervisor::test_live_usage_success(0),
        ),
        inspection_credit_inventory: OperationActivity::Loading,
        ..WorkflowActivities::default()
    };

    let mut revalidating = completed_inspection_activities();
    revalidating.revalidation_live_usage = OperationActivity::Loading;
    revalidating.revalidation_credit_inventory = OperationActivity::Refreshing {
        previous: Some(OperationSuccess::CreditInventory {
            credit_count: 1,
            usable_credit_count: 1,
        }),
    };

    let mut committing = completed_inspection_activities();
    committing.revalidation_live_usage =
        OperationActivity::Succeeded(crate::quota_reset::reset_session_supervisor::test_live_usage_success(0));
    committing.revalidation_credit_inventory =
        OperationActivity::Succeeded(OperationSuccess::CreditInventory {
            credit_count: 1,
            usable_credit_count: 1,
        });
    committing.consume_credit = OperationActivity::RequestDispatchedAwaitingOutcome;

    let mut known = committing.clone();
    known.consume_credit =
        OperationActivity::Succeeded(OperationSuccess::Consume(KnownConsumeOutcome::Reset {
            windows_reset: 2,
        }));

    let mut refused = completed_inspection_activities();
    refused.revalidation_live_usage = OperationActivity::Failed {
        failure: RenderSafeFailure::EligibilityRefused,
        previous: None,
    };
    refused.revalidation_credit_inventory =
        OperationActivity::Succeeded(OperationSuccess::CreditInventory {
            credit_count: 1,
            usable_credit_count: 1,
        });

    vec![
        (
            "inspecting-loading",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Inspecting,
                ConfirmationSelection::No,
                inspecting,
                None,
                None,
                Vec::new(),
                Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
            ),
            &["Weekly usage        ⠋ checking", "Reset credits       ⠋ checking"],
        ),
        (
            "inspection-partial",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Inspecting,
                ConfirmationSelection::No,
                inspection_partial,
                None,
                Some(test_live_weekly(0)),
                Vec::new(),
                Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
            ),
            &["Weekly usage        ready", "Reset credits       ⠋ checking"],
        ),
        (
            "confirming-ineligible",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Confirming,
                ConfirmationSelection::No,
                completed_inspection_activities(),
                None,
                Some(test_live_weekly(4)),
                test_credit_inventory(),
                Some(
                    ResetEligibilityDisabledReason::WeeklyRemainingNotBelowOnePercent {
                        remaining_percent: 4,
                    },
                ),
            ),
            &["[No]", "Yes disabled", "below 1% is required"],
        ),
        (
            "confirming-eligible",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Confirming,
                ConfirmationSelection::No,
                completed_inspection_activities(),
                None,
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &["[No]", "Yes", "Weekly remaining    0%"],
        ),
        (
            "revalidating-refreshing",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Revalidating,
                ConfirmationSelection::Yes,
                revalidating,
                None,
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
            ),
            &[
                "Weekly usage        ⠋ checking",
                "Reset credit        ⠋ refreshing · previous result visible",
                "Rechecking live eligibility",
            ],
        ),
        (
            "committing-dispatched",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Committing,
                ConfirmationSelection::Yes,
                committing,
                None,
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &[
                "Reset request sent",
                "Provider            ⠋ waiting for a definitive result",
            ],
        ),
        (
            "result-known",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Result,
                ConfirmationSelection::No,
                known,
                Some(WorkflowResult::Known(KnownConsumeOutcome::Reset {
                    windows_reset: 2,
                })),
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &["Success — reset completed", "2 quota windows reset", "One reset credit was consumed"],
        ),
        (
            "result-refused",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Result,
                ConfirmationSelection::No,
                refused,
                Some(WorkflowResult::Refused(
                    RenderSafeFailure::EligibilityRefused,
                )),
                Some(test_live_weekly(4)),
                test_credit_inventory(),
                Some(
                    ResetEligibilityDisabledReason::WeeklyRemainingNotBelowOnePercent {
                        remaining_percent: 4,
                    },
                ),
            ),
            &["Not consumed", "Reset refused before consume", "No consume request was sent"],
        ),
        (
            "result-unknown",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Result,
                ConfirmationSelection::No,
                unknown_result_activities(),
                Some(WorkflowResult::OutcomeUnknown(
                    ConsumeUnknownReason::Transport,
                )),
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &["Outcome unknown — do not retry", "The credit may have been consumed"],
        ),
    ]
}
fn completed_inspection_activities() -> WorkflowActivities {
    WorkflowActivities {
        inspection_live_usage: OperationActivity::Succeeded(
            crate::quota_reset::reset_session_supervisor::test_live_usage_success(0),
        ),
        inspection_credit_inventory: OperationActivity::Succeeded(
            OperationSuccess::CreditInventory {
            credit_count: 1,
            usable_credit_count: 1,
        },
        ),
        ..WorkflowActivities::default()
    }
}

fn unknown_result_activities() -> WorkflowActivities {
    let mut activities = completed_inspection_activities();
    activities.revalidation_live_usage =
        OperationActivity::Succeeded(crate::quota_reset::reset_session_supervisor::test_live_usage_success(0));
    activities.revalidation_credit_inventory =
        OperationActivity::Succeeded(OperationSuccess::CreditInventory {
            credit_count: 1,
            usable_credit_count: 1,
        });
    activities.consume_credit = OperationActivity::Failed {
        failure: RenderSafeFailure::Transport,
        previous: None,
    };
    activities
}

fn test_live_weekly(remaining_percent: u32) -> LiveWeeklyDisplayFacts {
    LiveWeeklyDisplayFacts {
        remaining_percent,
        provenance: ResetValueProvenance::CurrentLive,
    }
}

fn test_credit_inventory() -> Vec<ResetCreditDisplayRecord> {
    vec![ResetCreditDisplayRecord {
        id_hint: "abcd…wxyz".to_owned(),
        status: ResetCreditDisplayStatusDto::Available,
        title: Some("Weekly recovery".to_owned()),
        expires_unix_seconds: Some(1_900_000_000),
        earliest_usable: true,
    }]
}
