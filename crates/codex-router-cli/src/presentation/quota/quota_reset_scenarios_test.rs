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
            "inspected-below-ten-eligible",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Inspected,
                ConfirmationSelection::No,
                completed_inspection_activities(),
                None,
                Some(test_live_weekly(9)),
                test_credit_inventory(),
                None,
            ),
            &["Weekly remaining    9% · eligible"],
        ),
        (
            "confirming-ineligible",
            ResetWorkflowSnapshot::test_snapshot(
                WorkflowPhase::Confirming,
                ConfirmationSelection::No,
                completed_inspection_activities(),
                None,
                Some(test_live_weekly(10)),
                test_credit_inventory(),
                Some(
                    ResetEligibilityDisabledReason::WeeklyRemainingNotBelowTenPercentOrCreditNotExpiringSoon {
                        remaining_percent: 10,
                    },
                ),
            ),
            &[
                "[No]",
                "Yes disabled",
                "below 10% or credit expiry within 12h is required",
            ],
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
                Some(test_live_weekly(10)),
                test_credit_inventory(),
                Some(
                    ResetEligibilityDisabledReason::WeeklyRemainingNotBelowTenPercentOrCreditNotExpiringSoon {
                        remaining_percent: 10,
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


#[test]
fn account_options_resets_reserve_inventory_and_short_eligibility_rows() {
    let view_model = quota_two_account_view_model();
    let mut options =
        super::quota_account_options::AccountOptionsState::new(&view_model.rows[0], 1);
    options.tab = super::quota_account_options::AccountOptionsTab::Resets;
    let target = test_reset_target();
    let snapshot = ResetWorkflowSnapshot::test_snapshot(
        WorkflowPhase::Inspected,
        ConfirmationSelection::No,
        completed_inspection_activities(),
        None,
        Some(test_live_weekly(9)),
        test_credit_inventory(),
        None,
    );
    let options_height = super::quota_account_options::account_options_content_height(
        &options,
        Some(&snapshot),
        Some(&target),
        0,
    );

    let wide_frame = super::quota_account_options::render_account_options_panel(
        super::quota_account_options::AccountOptionsPanelProps {
            options: &options,
            reset_snapshot: Some(&snapshot),
            reset_target: Some(&target),
            width: 62,
            height: options_height,
            inventory_page_start: 0,
            spinner_tick: 0,
        },
    )
    .render(None)
    .to_string();
    assert!(wide_frame.contains("Live eligibility"), "{wide_frame}");
    assert!(wide_frame.contains("Usable credits"), "{wide_frame}");
    assert!(wide_frame.contains("Reset credits · live"), "{wide_frame}");
    assert!(wide_frame.contains("Weekly recovery"), "{wide_frame}");
    assert!(!wide_frame.contains("← Reset credit"), "{wide_frame}");

    let short_frame = super::quota_account_options::render_account_options_panel(
        super::quota_account_options::AccountOptionsPanelProps {
            options: &options,
            reset_snapshot: Some(&snapshot),
            reset_target: Some(&target),
            width: 96,
            height: 14,
            inventory_page_start: 0,
            spinner_tick: 0,
        },
    )
    .render(None)
    .to_string();
    assert!(short_frame.contains("Live eligibility"), "{short_frame}");
    assert!(short_frame.contains("Usable credits"), "{short_frame}");
    assert!(short_frame.contains("Weekly recovery"), "{short_frame}");
}

#[test]
fn reset_options_result_content_keeps_outcome_title() {
    let target = test_reset_target();
    let snapshot = ResetWorkflowSnapshot::test_snapshot(
        WorkflowPhase::Result,
        ConfirmationSelection::No,
        unknown_result_activities(),
        Some(WorkflowResult::OutcomeUnknown(ConsumeUnknownReason::Transport)),
        None,
        Vec::new(),
        None,
    );

    let frame = super::quota_reset_detail_rendering::render_reset_panel_content(
        &snapshot, &target, 56, 16, 0, 4, 0,
    )
    .render(None)
    .to_string();

    assert!(frame.contains("Outcome unknown — do not retry"), "{frame}");
    assert!(frame.contains("The credit may have been consumed"), "{frame}");
}
