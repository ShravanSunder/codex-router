fn reset_operation_scenarios() -> Vec<(&'static str, ResetWorkflowSnapshot, &'static [&'static str])>
{
    let mut inspecting = WorkflowActivities::default();
    inspecting.inspection_live_usage = OperationActivity::Loading;
    inspecting.inspection_credit_inventory = OperationActivity::Loading;

    let mut inspection_partial = WorkflowActivities::default();
    inspection_partial.inspection_live_usage =
        OperationActivity::Succeeded(crate::quota_reset::supervisor::test_live_usage_success(0));
    inspection_partial.inspection_credit_inventory = OperationActivity::Loading;

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
        OperationActivity::Succeeded(crate::quota_reset::supervisor::test_live_usage_success(0));
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
                false,
                inspecting,
                None,
                None,
                Vec::new(),
                Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
            ),
            &["inspect usage      loading", "inspect credits    loading"],
        ),
        (
            "inspection-partial",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Inspecting,
                ConfirmationSelection::No,
                false,
                inspection_partial,
                None,
                Some(test_live_weekly(0)),
                Vec::new(),
                Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
            ),
            &["inspect usage      succeeded", "inspect credits    loading"],
        ),
        (
            "confirming-ineligible",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Confirming,
                ConfirmationSelection::No,
                false,
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
            &["[No]", "Yes disabled", "less than 1% is required"],
        ),
        (
            "confirming-eligible",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Confirming,
                ConfirmationSelection::No,
                true,
                completed_inspection_activities(),
                None,
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &["[No]", " Yes ", "current live weekly  0% remaining"],
        ),
        (
            "revalidating-refreshing",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Revalidating,
                ConfirmationSelection::Yes,
                false,
                revalidating,
                None,
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
            ),
            &[
                "revalidate usage   loading",
                "revalidate credits refreshing · previous result visible",
                "Revalidating account",
            ],
        ),
        (
            "committing-dispatched",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Committing,
                ConfirmationSelection::Yes,
                false,
                committing,
                None,
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &[
                "consume credit     request dispatched · awaiting definitive outcome",
                "waiting for a definitive result",
            ],
        ),
        (
            "result-known",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Result,
                ConfirmationSelection::No,
                false,
                known,
                Some(WorkflowResult::Known(KnownConsumeOutcome::Reset {
                    windows_reset: 2,
                })),
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &[
                "consume credit     succeeded",
                "Reset completed: 2 windows reset",
            ],
        ),
        (
            "result-refused",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Result,
                ConfirmationSelection::No,
                false,
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
            &[
                "revalidate usage   failed: reset eligibility refused",
                "Reset refused before consume",
            ],
        ),
        (
            "result-unknown",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Result,
                ConfirmationSelection::No,
                false,
                unknown_result_activities(),
                Some(WorkflowResult::OutcomeUnknown(
                    ConsumeUnknownReason::Transport,
                )),
                Some(test_live_weekly(0)),
                test_credit_inventory(),
                None,
            ),
            &[
                "consume credit     failed",
                "Outcome unknown",
                "Do not retry automatically",
            ],
        ),
    ]
}
fn completed_inspection_activities() -> WorkflowActivities {
    let mut activities = WorkflowActivities::default();
    activities.inspection_live_usage =
        OperationActivity::Succeeded(crate::quota_reset::supervisor::test_live_usage_success(0));
    activities.inspection_credit_inventory =
        OperationActivity::Succeeded(OperationSuccess::CreditInventory {
            credit_count: 1,
            usable_credit_count: 1,
        });
    activities
}

fn unknown_result_activities() -> WorkflowActivities {
    let mut activities = completed_inspection_activities();
    activities.revalidation_live_usage =
        OperationActivity::Succeeded(crate::quota_reset::supervisor::test_live_usage_success(0));
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
