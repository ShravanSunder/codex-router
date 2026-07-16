use super::*;

#[test]
fn quota_status_formats_subpercent_burn_with_runout() {
    let estimate = QuotaRunRateEstimate::with_rate_basis_points_per_hour(
        QuotaRunRateConfidence::Normal,
        45,
        6,
    );

    assert_eq!(
        format_run_rate_estimate(estimate, NOW),
        "normal burn 0.45%/h; runout in 13h 20m"
    );
}

#[test]
fn quota_status_connection_rate_distinguishes_aggregate_only_burn() {
    let aggregate_only_snapshot = QuotaPaceSnapshot {
        remaining_headroom: 55,
        reset_unix_seconds: Some(NOW + V1_WEEKLY_WINDOW_SECONDS),
        projected_exhaustion_unix_seconds: Some(NOW + 36 * 60 * 60),
        projected_candidate_burn_basis_points_per_hour: Some(141),
        aggregate_burn_basis_points_per_hour: Some(141),
        per_connection_burn_basis_points_per_hour: None,
        confidence: QuotaRunRateConfidence::Low,
    };

    assert_eq!(
        quota_connection_rate_summary(Some(aggregate_only_snapshot)),
        "not attributed (low)"
    );
}

#[test]
fn quota_status_json_exposes_subpercent_burn_basis_points() {
    let estimate = QuotaRunRateEstimate::with_rate_basis_points_per_hour(
        QuotaRunRateConfidence::Normal,
        45,
        6,
    );

    let json = serde_json::to_value(JsonRunRateEstimate::from_estimate(estimate, NOW))
        .unwrap_or_else(|error| panic!("run-rate JSON should serialize: {error}"));

    assert_eq!(json["burn_rate_percent_per_hour"], 0);
    assert_eq!(json["burn_rate_basis_points_per_hour"], 45);
    assert!(json["projected_exhaustion_unix_seconds"].is_number());
}

#[test]
fn quota_status_sample_confidence_uses_15_minute_display_boundary() {
    assert_eq!(
        sample_metadata_from_observed_windows(&[NOW - 899], NOW).confidence,
        SampleConfidence::Fresh
    );
    assert_eq!(
        sample_metadata_from_observed_windows(&[NOW - 900], NOW).confidence,
        SampleConfidence::Fresh
    );
    assert_eq!(
        sample_metadata_from_observed_windows(&[NOW - 901], NOW).confidence,
        SampleConfidence::Stale
    );
}

#[test]
fn quota_status_sample_confidence_uses_displayed_value_window_age() {
    let windows = vec![
        DisplayQuotaWindow {
            observed_unix_seconds: NOW - 30,
            ..display_window(
                V1_SHORT_WINDOW_SECONDS,
                20,
                NOW + V1_SHORT_WINDOW_SECONDS,
                QuotaRunRateEstimate::unknown(),
            )
        },
        DisplayQuotaWindow {
            observed_unix_seconds: NOW - 901,
            ..display_window(
                V1_WEEKLY_WINDOW_SECONDS,
                70,
                NOW + V1_WEEKLY_WINDOW_SECONDS,
                QuotaRunRateEstimate::unknown(),
            )
        },
        DisplayQuotaWindow {
            status: QuotaWindowStatus::Unknown,
            observed_unix_seconds: NOW - 3_600,
            ..display_window(
                V1_WEEKLY_WINDOW_SECONDS * 2,
                0,
                NOW + V1_WEEKLY_WINDOW_SECONDS,
                QuotaRunRateEstimate::unknown(),
            )
        },
    ];

    let sample = sample_metadata_from_display_windows(&windows, NOW);

    assert_eq!(sample.confidence, SampleConfidence::Stale);
    assert_eq!(sample.age_seconds, Some(901));
    assert_eq!(sample.semantic_label, "sample stale");
}

#[test]
fn quota_status_row_sample_uses_only_weekly_window_age() {
    let mut report = quota_capture_report();
    let row = report
        .rows
        .get_mut(0)
        .unwrap_or_else(|| panic!("capture report should include a selected row"));
    for window in &mut row.windows {
        if window.window_seconds == V1_SHORT_WINDOW_SECONDS {
            window.observed_unix_seconds = NOW - 901;
        } else if window.window_seconds == V1_WEEKLY_WINDOW_SECONDS {
            window.observed_unix_seconds = NOW - 30;
        }
    }

    let view_model = quota_status_view_model(&report, report.rows(), 120);
    let rendered_row = view_model
        .rows
        .first()
        .unwrap_or_else(|| panic!("quota view model should include a row"));
    let selected = view_model
        .selected
        .as_ref()
        .unwrap_or_else(|| panic!("quota view model should include selected details"));

    assert_eq!(
        rendered_row.sample_metadata.confidence,
        SampleConfidence::Fresh
    );
    assert_eq!(rendered_row.sample_metadata.age_seconds, Some(30));
    assert_eq!(selected.sample_metadata.confidence, SampleConfidence::Stale);
    assert_eq!(selected.sample_metadata.age_seconds, Some(901));
}

#[test]
fn quota_status_reset_pace_classifies_thresholds() {
    for (multiple_basis_points, expected_state) in [
        (79, ResetPaceState::UnderBurning),
        (80, ResetPaceState::Healthy),
        (100, ResetPaceState::Healthy),
        (120, ResetPaceState::Healthy),
        (121, ResetPaceState::OverBurning),
    ] {
        let view_model =
            reset_pace_view_model_from_multiple_basis_points(Some(multiple_basis_points));

        assert_eq!(
            view_model.state, expected_state,
            "{multiple_basis_points} basis points should classify correctly"
        );
    }
}

#[test]
fn quota_status_reset_pace_meter_fills_from_center_by_direction() {
    for (multiple_basis_points, expected_meter) in [
        (9, "■■■■■■■│□□□□□□□"),
        (25, "□□■■■■■│□□□□□□□"),
        (50, "□□□□■■■│□□□□□□□"),
        (79, "□□□□□□■│□□□□□□□"),
        (80, "□□□□□□□│□□□□□□□"),
        (100, "□□□□□□□│□□□□□□□"),
        (120, "□□□□□□□│□□□□□□□"),
        (121, "□□□□□□□│■□□□□□□"),
        (150, "□□□□□□□│■■■□□□□"),
        (200, "□□□□□□□│■■■■■■■"),
    ] {
        let view_model =
            reset_pace_view_model_from_multiple_basis_points(Some(multiple_basis_points));

        assert_eq!(
            reset_pace_meter_text(&view_model),
            expected_meter,
            "{multiple_basis_points} reset-pace basis points should fill from the center in the matching direction"
        );
        assert_eq!(
            reset_pace_meter_text(&view_model).chars().count(),
            15,
            "reset-pace meter must always replace fixed slots, not add glyphs"
        );
    }
}

#[test]
fn quota_status_reset_pace_over_meter_uses_window_reset_denominator() {
    for (reset_seconds, projected_exhaustion_seconds, expected_meter) in [
        (
            V1_WEEKLY_WINDOW_SECONDS,
            2 * 24 * 60 * 60,
            "□□□□□□□│■■■■■□□",
        ),
        (V1_SHORT_WINDOW_SECONDS, 2 * 60 * 60, "□□□□□□□│■■■■■□□"),
    ] {
        let snapshot = QuotaPaceSnapshot {
            remaining_headroom: 10,
            reset_unix_seconds: Some(NOW + reset_seconds),
            projected_exhaustion_unix_seconds: Some(NOW + projected_exhaustion_seconds),
            projected_candidate_burn_basis_points_per_hour: Some(300),
            aggregate_burn_basis_points_per_hour: Some(300),
            per_connection_burn_basis_points_per_hour: None,
            confidence: QuotaRunRateConfidence::Low,
        };

        let view_model = reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

        assert_eq!(
            reset_pace_meter_text(&view_model),
            expected_meter,
            "over-pace meter should normalize early runout by this window's reset time"
        );
    }
}

#[test]
fn quota_status_reset_pace_over_two_x_shows_runout_impact() {
    let snapshot = QuotaPaceSnapshot {
        remaining_headroom: 10,
        reset_unix_seconds: Some(NOW + 10 * 60 * 60),
        projected_exhaustion_unix_seconds: Some(NOW + 3 * 60 * 60),
        projected_candidate_burn_basis_points_per_hour: Some(300),
        aggregate_burn_basis_points_per_hour: Some(300),
        per_connection_burn_basis_points_per_hour: None,
        confidence: QuotaRunRateConfidence::Low,
    };

    let view_model = reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

    assert_eq!(view_model.multiple_label, "3.00x reset pace");
    assert_eq!(view_model.impact_label, Some("runs out 3h".to_owned()));
    assert_eq!(plain_reset_pace_summary(&view_model), "runs out 3h");
}

#[test]
fn quota_status_reset_pace_at_two_x_keeps_multiplier_label() {
    let snapshot = QuotaPaceSnapshot {
        remaining_headroom: 10,
        reset_unix_seconds: Some(NOW + 10 * 60 * 60),
        projected_exhaustion_unix_seconds: Some(NOW + 5 * 60 * 60),
        projected_candidate_burn_basis_points_per_hour: Some(200),
        aggregate_burn_basis_points_per_hour: Some(200),
        per_connection_burn_basis_points_per_hour: None,
        confidence: QuotaRunRateConfidence::Low,
    };

    let view_model = reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

    assert_eq!(view_model.multiple_label, "2.00x reset pace");
    assert_eq!(view_model.impact_label, None);
}

#[test]
fn quota_status_reset_pace_unavailable_has_marker_meter() {
    let view_model = reset_pace_view_model_from_multiple_basis_points(None);

    assert_eq!(view_model.state, ResetPaceState::Unavailable);
    assert_eq!(view_model.semantic_label, "burn unavailable");
    assert_eq!(view_model.meter_left_segments.filled, 0);
    assert_eq!(view_model.meter_right_segments.filled, 0);
    assert_eq!(view_model.meter_left_segments.empty, 7);
    assert_eq!(view_model.meter_right_segments.empty, 7);
    assert_eq!(view_model.center_marker, '│');
    assert!(view_model.unavailable_reason.is_some());
}

#[test]
fn quota_status_short_reset_pace_collects_data_for_insufficient_snapshot_samples() {
    let snapshot = QuotaPaceSnapshot {
        remaining_headroom: 99,
        reset_unix_seconds: Some(NOW + V1_SHORT_WINDOW_SECONDS),
        projected_exhaustion_unix_seconds: None,
        projected_candidate_burn_basis_points_per_hour: None,
        aggregate_burn_basis_points_per_hour: None,
        per_connection_burn_basis_points_per_hour: None,
        confidence: QuotaRunRateConfidence::Insufficient,
    };

    let view_model = short_reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

    assert_eq!(view_model.state, ResetPaceState::Unavailable);
    assert_eq!(view_model.semantic_label, "collecting data");
    assert_eq!(reset_pace_meter_text(&view_model), "□□□□□□□│□□□□□□□");
}

#[test]
fn quota_status_display_reset_pace_requires_three_recent_samples() {
    let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
    let observations = [
        QuotaRunRateObservation::new(NOW - 899, reset_unix_seconds, 50),
        QuotaRunRateObservation::new(NOW - 600, reset_unix_seconds, 48),
    ];

    let display_estimate = display_quota_run_rate_estimate(
        V1_WEEKLY_WINDOW_SECONDS,
        NOW,
        reset_unix_seconds,
        &observations,
    );
    let routing_authority_estimate = QuotaRunRateEstimator::new(
        DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS,
    )
    .estimate(NOW, reset_unix_seconds, &observations);

    assert_eq!(
        display_estimate.confidence(),
        QuotaRunRateConfidence::Insufficient
    );
    assert!(display_estimate.burn_rate_basis_points_per_hour().is_none());
    assert_eq!(
        routing_authority_estimate.confidence(),
        QuotaRunRateConfidence::Stale,
        "runtime authority must still go stale at the persisted 300s boundary"
    );
}

#[test]
fn quota_status_display_burn_uses_recent_window_and_sample_confidence() {
    let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
    let observations = [
        QuotaRunRateObservation::new(NOW - 20_000, reset_unix_seconds, 100),
        QuotaRunRateObservation::new(NOW - 19_000, reset_unix_seconds, 50),
        QuotaRunRateObservation::new(NOW - 3_000, reset_unix_seconds, 50),
        QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 49),
        QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 48),
        QuotaRunRateObservation::new(NOW - 300, reset_unix_seconds, 47),
    ];

    let estimate = display_quota_run_rate_estimate(
        V1_WEEKLY_WINDOW_SECONDS,
        NOW,
        reset_unix_seconds,
        &observations,
    );

    assert_eq!(estimate.confidence(), QuotaRunRateConfidence::Low);
    assert_eq!(estimate.burn_rate_basis_points_per_hour(), Some(400));
}

#[test]
fn quota_status_display_burn_requires_five_recent_samples_for_normal_confidence() {
    let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
    let four_observations = [
        QuotaRunRateObservation::new(NOW - 2_700, reset_unix_seconds, 50),
        QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 49),
        QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 48),
        QuotaRunRateObservation::new(NOW, reset_unix_seconds, 47),
    ];
    let five_observations = [
        QuotaRunRateObservation::new(NOW - 3_600, reset_unix_seconds, 51),
        QuotaRunRateObservation::new(NOW - 2_700, reset_unix_seconds, 50),
        QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 49),
        QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 48),
        QuotaRunRateObservation::new(NOW, reset_unix_seconds, 47),
    ];

    let four_sample_estimate = display_quota_run_rate_estimate(
        V1_WEEKLY_WINDOW_SECONDS,
        NOW,
        reset_unix_seconds,
        &four_observations,
    );
    let five_sample_estimate = display_quota_run_rate_estimate(
        V1_WEEKLY_WINDOW_SECONDS,
        NOW,
        reset_unix_seconds,
        &five_observations,
    );

    assert_eq!(
        four_sample_estimate.confidence(),
        QuotaRunRateConfidence::Low
    );
    assert_eq!(
        five_sample_estimate.confidence(),
        QuotaRunRateConfidence::Normal
    );
}

#[test]
fn quota_status_display_burn_uses_all_recent_samples() {
    let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
    let observations = [
        QuotaRunRateObservation::new(NOW - 9_000, reset_unix_seconds, 80),
        QuotaRunRateObservation::new(NOW - 3_600, reset_unix_seconds, 54),
        QuotaRunRateObservation::new(NOW - 2_700, reset_unix_seconds, 53),
        QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 52),
        QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 51),
        QuotaRunRateObservation::new(NOW, reset_unix_seconds, 50),
    ];

    let estimate = display_quota_run_rate_estimate(
        V1_WEEKLY_WINDOW_SECONDS,
        NOW,
        reset_unix_seconds,
        &observations,
    );

    assert_eq!(estimate.confidence(), QuotaRunRateConfidence::Normal);
    assert_eq!(
        estimate.burn_rate_basis_points_per_hour(),
        Some(1_200),
        "display burn should use every sample inside the recent lookback, not only the newest five samples"
    );
}

#[test]
fn quota_status_display_reset_pace_uses_display_estimate_when_projection_is_stale() {
    let windows = vec![display_window(
        V1_WEEKLY_WINDOW_SECONDS,
        50,
        NOW + V1_WEEKLY_WINDOW_SECONDS,
        QuotaRunRateEstimate::with_rate_basis_points_per_hour(
            QuotaRunRateConfidence::Low,
            2_000,
            50,
        ),
    )];
    let stale_projected_weekly_window =
        QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Stale)
            .with_remaining_headroom(50)
            .with_reset_unix_seconds(NOW + V1_WEEKLY_WINDOW_SECONDS)
            .with_observed_unix_seconds(NOW - 301)
            .with_burn_rate_confidence(QuotaRunRateConfidence::Stale);

    let snapshot = quota_pace_snapshot(&windows, Some(&stale_projected_weekly_window), NOW)
        .unwrap_or_else(|| panic!("weekly display window should produce pace snapshot"));

    assert_eq!(snapshot.aggregate_burn_basis_points_per_hour, Some(2_000));
    assert_eq!(
        snapshot.projected_candidate_burn_basis_points_per_hour,
        Some(2_000)
    );
    assert_eq!(snapshot.confidence, QuotaRunRateConfidence::Low);
}

#[test]
fn quota_status_shared_dto_carries_sample_and_reset_pace_without_string_parsing() {
    let report = quota_capture_report();

    let view_model = quota_status_view_model(&report, report.rows(), 120);
    let row = view_model
        .rows
        .first()
        .unwrap_or_else(|| panic!("quota view model should include an account row"));

    assert_eq!(row.account_id, report.rows()[0].account_id);
    assert_eq!(row.active_credential_generation, Some(1));
    assert_eq!(row.sample_metadata.confidence, SampleConfidence::Fresh);
    assert_eq!(row.sample_metadata.semantic_label, "sample fresh");
    assert_ne!(row.reset_pace.state, ResetPaceState::Unavailable);
    assert!(
        row.reset_pace.multiple_label.contains("reset pace"),
        "reset pace should be carried as typed row metadata, not rebuilt from safe-pace strings"
    );
    assert!(
        row.burn_meter.contains('│'),
        "row burn meter should use the same center-out reset-pace meter as the visible reset pace"
    );
    assert!(
        !row.weekly_pace.contains("safe pace"),
        "legacy safe-pace copy must not survive in the shared DTO"
    );
}

#[test]
fn quota_status_selected_details_carry_5h_reset_pace_from_short_window() {
    let mut report = quota_capture_report();
    let selected_row = report
        .rows
        .first_mut()
        .unwrap_or_else(|| panic!("capture report should include selected row"));
    let short_window = selected_row
        .windows
        .iter_mut()
        .find(|window| window.window_seconds == V1_SHORT_WINDOW_SECONDS)
        .unwrap_or_else(|| panic!("selected row should include a short window"));
    short_window.run_rate_estimate = QuotaRunRateEstimate::with_rate_basis_points_per_hour(
        QuotaRunRateConfidence::Low,
        5_000,
        short_window.remaining_headroom,
    );

    let view_model = quota_status_view_model(&report, report.rows(), 120);
    let selected = view_model
        .selected
        .as_ref()
        .unwrap_or_else(|| panic!("quota view model should include selected details"));

    assert_eq!(selected.short_reset_pace.state, ResetPaceState::OverBurning);
    assert!(
        selected
            .short_reset_pace
            .impact_label
            .as_deref()
            .is_some_and(|label| label.starts_with("runs out ")),
        "5h reset pace should carry its own runout impact: {:?}",
        selected.short_reset_pace
    );
}
