use super::*;

pub(super) fn quota_pace_snapshot(
    windows: &[DisplayQuotaWindow],
    projected_weekly_window: Option<&QuotaWindowFact>,
    now_unix_seconds: u64,
) -> Option<QuotaPaceSnapshot> {
    quota_pace_snapshot_for_window(
        windows,
        V1_WEEKLY_WINDOW_SECONDS,
        projected_weekly_window,
        now_unix_seconds,
    )
}

pub(super) fn quota_display_pace_snapshot(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    now_unix_seconds: u64,
) -> Option<QuotaPaceSnapshot> {
    quota_pace_snapshot_for_window(windows, window_seconds, None, now_unix_seconds)
}

pub(super) fn quota_pace_snapshot_for_window(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    projected_window: Option<&QuotaWindowFact>,
    now_unix_seconds: u64,
) -> Option<QuotaPaceSnapshot> {
    let display_window = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)?;
    let projected_aggregate_rate =
        projected_window.and_then(QuotaWindowFact::aggregate_burn_basis_points_per_hour);
    let display_aggregate_rate = display_window
        .run_rate_estimate
        .burn_rate_basis_points_per_hour();
    let aggregate_rate = projected_aggregate_rate.or(display_aggregate_rate);
    let projected_candidate_rate =
        projected_window.and_then(QuotaWindowFact::projected_candidate_burn_basis_points_per_hour);
    let candidate_rate = projected_candidate_rate.or(aggregate_rate);
    let projected_exhaustion_from_projection =
        projected_window.and_then(QuotaWindowFact::projected_exhaustion_unix_seconds);
    let display_exhaustion = display_window
        .run_rate_estimate
        .projected_exhaustion_unix_seconds(now_unix_seconds);
    let projected_exhaustion = projected_exhaustion_from_projection.or(display_exhaustion);
    let projection_has_burn_estimate = projected_aggregate_rate.is_some()
        || projected_candidate_rate.is_some()
        || projected_exhaustion_from_projection.is_some();
    let confidence = if projection_has_burn_estimate {
        projected_window.map_or(
            display_window.run_rate_estimate.confidence(),
            QuotaWindowFact::burn_rate_confidence,
        )
    } else {
        display_window.run_rate_estimate.confidence()
    };
    Some(QuotaPaceSnapshot {
        remaining_headroom: display_window.remaining_headroom,
        reset_unix_seconds: display_window.reset_unix_seconds,
        projected_exhaustion_unix_seconds: projected_exhaustion,
        projected_candidate_burn_basis_points_per_hour: candidate_rate,
        aggregate_burn_basis_points_per_hour: aggregate_rate,
        per_connection_burn_basis_points_per_hour: projected_window
            .and_then(QuotaWindowFact::per_connection_burn_basis_points_per_hour),
        confidence,
    })
}

pub(super) fn quota_pace_summary(
    snapshot: Option<QuotaPaceSnapshot>,
    now_unix_seconds: u64,
) -> String {
    let Some(snapshot) = snapshot else {
        return "burn unavailable".to_owned();
    };
    let direction = quota_pace_direction(snapshot, now_unix_seconds);
    let reset_pace = reset_pace_view_model_from_snapshot(Some(snapshot), now_unix_seconds);
    if reset_pace.state == ResetPaceState::Unavailable {
        return format!("{direction}  burn unavailable");
    }
    format!(
        "{direction}  {} {}",
        reset_pace.multiple_label, reset_pace.semantic_label
    )
}

pub(super) fn quota_total_rate_summary(snapshot: Option<QuotaPaceSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "rate unknown".to_owned();
    };
    let total_rate = snapshot
        .projected_candidate_burn_basis_points_per_hour
        .or(snapshot.aggregate_burn_basis_points_per_hour)
        .map_or_else(
            || "unknown".to_owned(),
            format_burn_rate_basis_points_per_hour,
        );
    format!(
        "{total_rate} total ({})",
        run_rate_confidence_label(snapshot.confidence)
    )
}

pub(super) fn quota_compact_total_burn_rate(snapshot: Option<QuotaPaceSnapshot>) -> Option<String> {
    let snapshot = snapshot?;
    snapshot
        .projected_candidate_burn_basis_points_per_hour
        .or(snapshot.aggregate_burn_basis_points_per_hour)
        .map(format_burn_rate_basis_points_per_hour)
        .map(|rate| format!("burn {rate}"))
}

pub(super) fn quota_connection_rate_summary(snapshot: Option<QuotaPaceSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "connection rate unknown".to_owned();
    };
    let Some(per_connection_rate) = snapshot.per_connection_burn_basis_points_per_hour else {
        if snapshot.aggregate_burn_basis_points_per_hour.is_some()
            || snapshot
                .projected_candidate_burn_basis_points_per_hour
                .is_some()
        {
            return format!(
                "not attributed ({})",
                run_rate_confidence_label(snapshot.confidence)
            );
        }
        return format!(
            "unknown ({})",
            run_rate_confidence_label(snapshot.confidence)
        );
    };
    format!(
        "{}/conn ({})",
        format_burn_rate_basis_points_per_hour(per_connection_rate),
        run_rate_confidence_label(snapshot.confidence)
    )
}

pub(super) fn quota_pace_direction(snapshot: QuotaPaceSnapshot, now_unix_seconds: u64) -> String {
    match (
        snapshot.projected_exhaustion_unix_seconds,
        snapshot.reset_unix_seconds,
    ) {
        (Some(projected_exhaustion), Some(reset)) if projected_exhaustion < reset => {
            format!(
                "behind {}",
                format_duration(reset.saturating_sub(projected_exhaustion))
            )
        }
        (Some(projected_exhaustion), Some(reset)) => {
            format!(
                "ahead {}",
                format_duration(projected_exhaustion.saturating_sub(reset))
            )
        }
        (None, Some(reset)) => format!(
            "ahead to reset ({})",
            format_relative_time(reset, now_unix_seconds)
        ),
        _ => "pace unknown".to_owned(),
    }
}

pub(super) fn quota_safe_pace_meter(
    snapshot: Option<QuotaPaceSnapshot>,
    now_unix_seconds: u64,
) -> String {
    let reset_pace = reset_pace_view_model_from_snapshot(snapshot, now_unix_seconds);
    reset_pace_meter_text(&reset_pace)
}

pub(super) fn quota_pace_load(snapshot: QuotaPaceSnapshot, now_unix_seconds: u64) -> Option<u32> {
    let reset_unix_seconds = snapshot.reset_unix_seconds?;
    if snapshot.remaining_headroom == 0 {
        return Some(999);
    }
    let time_left_seconds = reset_unix_seconds.saturating_sub(now_unix_seconds);
    if time_left_seconds == 0 {
        return None;
    }
    let candidate_rate = u128::from(snapshot.projected_candidate_burn_basis_points_per_hour?);
    let safe_rate = u128::from(snapshot.remaining_headroom)
        .saturating_mul(100)
        .saturating_mul(3_600)
        .checked_div(u128::from(time_left_seconds))?;
    if safe_rate == 0 {
        return None;
    }
    Some(((candidate_rate.saturating_mul(100)) / safe_rate).min(999) as u32)
}

pub(super) fn sample_metadata_from_display_windows(
    windows: &[DisplayQuotaWindow],
    now_unix_seconds: u64,
) -> SampleMetadata {
    let observed_unix_seconds = windows
        .iter()
        .filter(|window| !matches!(window.status, QuotaWindowStatus::Unknown))
        .map(|window| window.observed_unix_seconds)
        .collect::<Vec<_>>();
    sample_metadata_from_observed_windows(&observed_unix_seconds, now_unix_seconds)
}

pub(super) fn sample_metadata_from_display_window(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    now_unix_seconds: u64,
) -> SampleMetadata {
    let observed_unix_seconds = windows
        .iter()
        .filter(|window| {
            window.window_seconds == window_seconds
                && !matches!(window.status, QuotaWindowStatus::Unknown)
        })
        .map(|window| window.observed_unix_seconds)
        .collect::<Vec<_>>();
    sample_metadata_from_observed_windows(&observed_unix_seconds, now_unix_seconds)
}

pub(super) fn sample_metadata_from_observed_windows(
    observed_unix_seconds: &[u64],
    now_unix_seconds: u64,
) -> SampleMetadata {
    let Some(oldest_observed_unix_seconds) = observed_unix_seconds.iter().min().copied() else {
        return SampleMetadata::default();
    };
    let age_seconds = now_unix_seconds.saturating_sub(oldest_observed_unix_seconds);
    let confidence = if age_seconds <= QUOTA_STATUS_SAMPLE_FRESH_SECONDS {
        SampleConfidence::Fresh
    } else {
        SampleConfidence::Stale
    };
    let semantic_label = match confidence {
        SampleConfidence::Fresh => "sample fresh",
        SampleConfidence::Stale => "sample stale",
        SampleConfidence::Unknown => "sample unknown",
    };
    SampleMetadata {
        confidence,
        age_label: format_duration(age_seconds),
        age_seconds: Some(age_seconds),
        semantic_label,
    }
}

pub(super) fn reset_pace_view_model_from_snapshot(
    snapshot: Option<QuotaPaceSnapshot>,
    now_unix_seconds: u64,
) -> ResetPaceViewModel {
    let Some(snapshot) = snapshot else {
        return reset_pace_view_model_from_multiple_basis_points(None);
    };
    let multiple_hundredths = quota_pace_load(snapshot, now_unix_seconds);
    let impact_label = reset_pace_impact_label(snapshot, multiple_hundredths, now_unix_seconds);
    let mut view_model = reset_pace_view_model_from_multiple_basis_points(multiple_hundredths);
    if let Some(multiple_hundredths) = multiple_hundredths {
        let (left_filled, right_filled) =
            reset_pace_meter_fill_for_snapshot(snapshot, multiple_hundredths, now_unix_seconds);
        view_model.meter_left_segments = ResetPaceMeterSegments {
            filled: left_filled,
            empty: 7_usize.saturating_sub(left_filled),
        };
        view_model.meter_right_segments = ResetPaceMeterSegments {
            filled: right_filled,
            empty: 7_usize.saturating_sub(right_filled),
        };
    }
    view_model.impact_label = impact_label;
    view_model
}

pub(super) fn short_reset_pace_view_model_from_snapshot(
    snapshot: Option<QuotaPaceSnapshot>,
    now_unix_seconds: u64,
) -> ResetPaceViewModel {
    let mut view_model = reset_pace_view_model_from_snapshot(snapshot, now_unix_seconds);
    if view_model.state == ResetPaceState::Unavailable
        && snapshot
            .is_some_and(|snapshot| snapshot.confidence == QuotaRunRateConfidence::Insufficient)
    {
        view_model.semantic_label = "collecting data";
        view_model.multiple_label = "collecting data".to_owned();
        view_model.unavailable_reason = Some("collecting quota samples".to_owned());
    }
    view_model
}

pub(super) fn reset_pace_impact_label(
    snapshot: QuotaPaceSnapshot,
    multiple_hundredths: Option<u32>,
    now_unix_seconds: u64,
) -> Option<String> {
    if snapshot.remaining_headroom == 0 && snapshot.reset_unix_seconds.is_some() {
        return Some(DEPLETED_QUOTA_LABEL.to_owned());
    }
    if multiple_hundredths? <= RESET_PACE_RUNOUT_LABEL_THRESHOLD_HUNDREDTHS {
        return None;
    }
    let projected_exhaustion_unix_seconds = snapshot.projected_exhaustion_unix_seconds?;
    if projected_exhaustion_unix_seconds >= snapshot.reset_unix_seconds? {
        return None;
    }
    if projected_exhaustion_unix_seconds <= now_unix_seconds {
        return Some("runs out now".to_owned());
    }

    Some(format!(
        "runs out {}",
        format_duration(projected_exhaustion_unix_seconds.saturating_sub(now_unix_seconds))
    ))
}

pub(super) fn reset_pace_view_model_from_multiple_basis_points(
    multiple_hundredths: Option<u32>,
) -> ResetPaceViewModel {
    let Some(multiple_hundredths) = multiple_hundredths else {
        return ResetPaceViewModel::default();
    };
    let state = match multiple_hundredths {
        0..=79 => ResetPaceState::UnderBurning,
        80..=120 => ResetPaceState::Healthy,
        _ => ResetPaceState::OverBurning,
    };
    let semantic_label = match state {
        ResetPaceState::UnderBurning => "under",
        ResetPaceState::Healthy => "healthy",
        ResetPaceState::OverBurning => "over",
        ResetPaceState::Unavailable => "burn unavailable",
    };
    let (left_filled, right_filled) = reset_pace_meter_fill(multiple_hundredths);
    ResetPaceViewModel {
        state,
        multiple_label: format_reset_pace_multiple_label(multiple_hundredths),
        impact_label: None,
        semantic_label,
        meter_left_segments: ResetPaceMeterSegments {
            filled: left_filled,
            empty: 7_usize.saturating_sub(left_filled),
        },
        meter_right_segments: ResetPaceMeterSegments {
            filled: right_filled,
            empty: 7_usize.saturating_sub(right_filled),
        },
        center_marker: '│',
        unavailable_reason: None,
    }
}

pub(super) fn reset_pace_meter_text(reset_pace: &ResetPaceViewModel) -> String {
    reset_pace_meter_slots(
        reset_pace.meter_left_segments.filled,
        reset_pace.center_marker,
        reset_pace.meter_right_segments.filled,
    )
}

pub(super) fn reset_pace_meter_slots(
    left_filled: usize,
    center_marker: char,
    right_filled: usize,
) -> String {
    pub(super) const RESET_PACE_METER_SIDE_WIDTH: usize = 7;
    pub(super) const RESET_PACE_METER_EMPTY: char = '□';
    pub(super) const RESET_PACE_METER_FILLED: char = '■';
    let mut left_slots = [RESET_PACE_METER_EMPTY; RESET_PACE_METER_SIDE_WIDTH];
    let mut right_slots = [RESET_PACE_METER_EMPTY; RESET_PACE_METER_SIDE_WIDTH];
    for slot in left_slots
        .iter_mut()
        .rev()
        .take(left_filled.min(RESET_PACE_METER_SIDE_WIDTH))
    {
        *slot = RESET_PACE_METER_FILLED;
    }
    for slot in right_slots
        .iter_mut()
        .take(right_filled.min(RESET_PACE_METER_SIDE_WIDTH))
    {
        *slot = RESET_PACE_METER_FILLED;
    }

    left_slots
        .into_iter()
        .chain(std::iter::once(center_marker))
        .chain(right_slots)
        .collect()
}

pub(super) fn reset_pace_meter_fill(multiple_hundredths: u32) -> (usize, usize) {
    pub(super) const HEALTHY_LOWER_BOUND_HUNDREDTHS: u32 = 80;
    pub(super) const HEALTHY_UPPER_BOUND_HUNDREDTHS: u32 = 120;
    pub(super) const METER_SIDE_WIDTH: u32 = 7;

    if multiple_hundredths < HEALTHY_LOWER_BOUND_HUNDREDTHS {
        let under_distance = HEALTHY_LOWER_BOUND_HUNDREDTHS.saturating_sub(multiple_hundredths);
        (
            under_distance
                .saturating_mul(METER_SIDE_WIDTH)
                .div_ceil(HEALTHY_LOWER_BOUND_HUNDREDTHS) as usize,
            0,
        )
    } else if multiple_hundredths > HEALTHY_UPPER_BOUND_HUNDREDTHS {
        let over_distance = multiple_hundredths
            .saturating_sub(HEALTHY_UPPER_BOUND_HUNDREDTHS)
            .min(HEALTHY_LOWER_BOUND_HUNDREDTHS);
        (
            0,
            over_distance
                .saturating_mul(METER_SIDE_WIDTH)
                .div_ceil(HEALTHY_LOWER_BOUND_HUNDREDTHS) as usize,
        )
    } else {
        (0, 0)
    }
}

pub(super) fn reset_pace_meter_fill_for_snapshot(
    snapshot: QuotaPaceSnapshot,
    multiple_hundredths: u32,
    now_unix_seconds: u64,
) -> (usize, usize) {
    let Some(reset_unix_seconds) = snapshot.reset_unix_seconds else {
        return reset_pace_meter_fill(multiple_hundredths);
    };
    let Some(projected_exhaustion_unix_seconds) = snapshot.projected_exhaustion_unix_seconds else {
        return reset_pace_meter_fill(multiple_hundredths);
    };
    if projected_exhaustion_unix_seconds >= reset_unix_seconds {
        return reset_pace_meter_fill(multiple_hundredths);
    }
    let time_until_reset_seconds = reset_unix_seconds.saturating_sub(now_unix_seconds);
    if time_until_reset_seconds == 0 {
        return reset_pace_meter_fill(multiple_hundredths);
    }
    let early_by_seconds =
        reset_unix_seconds.saturating_sub(projected_exhaustion_unix_seconds.max(now_unix_seconds));
    (
        0,
        early_by_seconds
            .saturating_mul(7)
            .div_ceil(time_until_reset_seconds) as usize,
    )
}

pub(super) fn format_reset_pace_multiple_label(multiple_hundredths: u32) -> String {
    format!(
        "{}.{:02}x reset pace",
        multiple_hundredths / 100,
        multiple_hundredths % 100
    )
}
