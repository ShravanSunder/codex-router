use super::*;

pub(super) struct QuotaCaptureRowFixture {
    pub(super) account_id_value: &'static str,
    pub(super) account_label: &'static str,
    pub(super) preferred_next: bool,
    pub(super) short_remaining: u32,
    pub(super) weekly_remaining: u32,
    pub(super) freshness: QuotaEvidenceFreshness,
    pub(super) availability: AccountAvailability,
    pub(super) routing_reason: RoutingReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QuotaCaptureDesignCase {
    FreshHealthy,
    StaleUnder,
    DegradedOver,
    UnavailableBurn,
}

impl QuotaCaptureDesignCase {
    pub(super) const ALL: [Self; 4] = [
        Self::FreshHealthy,
        Self::StaleUnder,
        Self::DegradedOver,
        Self::UnavailableBurn,
    ];

    pub(super) const fn file_stem(self) -> &'static str {
        match self {
            Self::FreshHealthy => "fresh-healthy",
            Self::StaleUnder => "stale-under",
            Self::DegradedOver => "degraded-over",
            Self::UnavailableBurn => "unavailable-burn",
        }
    }
}

pub(super) fn quota_capture_row(fixture: QuotaCaptureRowFixture) -> QuotaStatusRow {
    let windows = vec![
        display_window(
            V1_SHORT_WINDOW_SECONDS,
            fixture.short_remaining,
            NOW + V1_SHORT_WINDOW_SECONDS,
            QuotaRunRateEstimate::unknown(),
        ),
        display_window(
            V1_WEEKLY_WINDOW_SECONDS,
            fixture.weekly_remaining,
            NOW + V1_WEEKLY_WINDOW_SECONDS,
            QuotaRunRateEstimate::unknown(),
        ),
    ];
    let quota_evidence_reason = if fixture.freshness == QuotaEvidenceFreshness::Stale {
        QuotaEvidenceReason::WindowExhausted
    } else if fixture.routing_reason == RoutingReason::HeldShortWindowGuard {
        QuotaEvidenceReason::ShortWindowGuard
    } else {
        QuotaEvidenceReason::Ok
    };

    QuotaStatusRow {
        account_id: account_id(fixture.account_id_value),
        active_credential_generation: Some(1),
        account_label: fixture.account_label.to_owned(),
        account_status: "enabled".to_owned(),
        short_window: format_window_cell(&windows, V1_SHORT_WINDOW_SECONDS, NOW, false),
        weekly_window: format_window_cell(&windows, V1_WEEKLY_WINDOW_SECONDS, NOW, false),
        pace: "history unknown".to_owned(),
        burn: "quota guard 5h 0% / weekly 8%".to_owned(),
        updated: if fixture.freshness == QuotaEvidenceFreshness::Stale {
            "failed 42m ago: network".to_owned()
        } else {
            "ok 14s ago".to_owned()
        },
        active_clients: "0 clients\nmirror <= 2h".to_owned(),
        active_clients_value: Some(
            if fixture.routing_reason == RoutingReason::HeldShortWindowGuard {
                5
            } else {
                0
            },
        ),
        active_clients_source: "sqlx_mirror",
        reset_credits_available: "2 available".to_owned(),
        reset_credits_available_value: Some(2),
        routing: format_routing_reason(fixture.routing_reason).to_owned(),
        next_use: format_next_use_for_capture(fixture.routing_reason).to_owned(),
        weekly_pace: Some(QuotaPaceSnapshot {
            remaining_headroom: fixture.weekly_remaining,
            reset_unix_seconds: Some(NOW + V1_WEEKLY_WINDOW_SECONDS),
            projected_exhaustion_unix_seconds: Some(
                NOW + u64::from(fixture.weekly_remaining)
                    .saturating_mul(100)
                    .saturating_mul(3_600)
                    / 10,
            ),
            projected_candidate_burn_basis_points_per_hour: Some(10),
            aggregate_burn_basis_points_per_hour: Some(8),
            per_connection_burn_basis_points_per_hour: Some(5),
            confidence: QuotaRunRateConfidence::Normal,
        }),
        windows,
        availability: fixture.availability,
        freshness: fixture.freshness,
        routing_exclusion: RoutingExclusion::None,
        quota_evidence_reason,
        routing_reason: fixture.routing_reason,
        preferred_next: fixture.preferred_next,
        short_pressure: 0,
        long_pressure: 8,
        short_salvage: fixture.short_remaining,
        long_salvage: fixture.weekly_remaining,
        limiting_window: None,
        weekly_survival_margin_basis_points: None,
        weekly_projected_exhaustion_unix_seconds: None,
        weekly_burn_rate_confidence: QuotaRunRateConfidence::Unknown,
    }
}

pub(super) fn quota_capture_report() -> QuotaStatusReport {
    QuotaStatusReport {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        route_band: "responses".to_owned(),
        selected_pool: SelectedPool::Usable,
        preferred_next_account_id: Some(account_id("acct_ssdev")),
        selection_projection_source: SelectionProjectionSource::SqlxProjection,
        now_unix_seconds: NOW,
        rows: vec![
            quota_capture_row(QuotaCaptureRowFixture {
                account_id_value: "acct_ssdev",
                account_label: "ssdev",
                preferred_next: true,
                short_remaining: 99,
                weekly_remaining: 83,
                freshness: QuotaEvidenceFreshness::Fresh,
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::PreferredSafestQuota,
            }),
            quota_capture_row(QuotaCaptureRowFixture {
                account_id_value: "acct_askluna",
                account_label: "askluna",
                preferred_next: false,
                short_remaining: 100,
                weekly_remaining: 99,
                freshness: QuotaEvidenceFreshness::Fresh,
                availability: AccountAvailability::Usable,
                routing_reason: RoutingReason::AvailableSamePool,
            }),
            quota_capture_row(QuotaCaptureRowFixture {
                account_id_value: "acct_matches",
                account_label: "matches",
                preferred_next: false,
                short_remaining: 94,
                weekly_remaining: 94,
                freshness: QuotaEvidenceFreshness::Fresh,
                availability: AccountAvailability::Reserve,
                routing_reason: RoutingReason::HeldShortWindowGuard,
            }),
            quota_capture_row(QuotaCaptureRowFixture {
                account_id_value: "acct_legacy",
                account_label: "legacy",
                preferred_next: false,
                short_remaining: 0,
                weekly_remaining: 0,
                freshness: QuotaEvidenceFreshness::Stale,
                availability: AccountAvailability::Blocked,
                routing_reason: RoutingReason::BlockedWindowExhausted,
            }),
        ],
    }
}

pub(super) fn quota_capture_case_report(case: QuotaCaptureDesignCase) -> QuotaStatusReport {
    let mut report = quota_capture_report();
    match case {
        QuotaCaptureDesignCase::FreshHealthy => {
            let selected_row = report
                .rows
                .get_mut(0)
                .unwrap_or_else(|| panic!("capture report should include selected row"));
            selected_row.weekly_pace = selected_row.weekly_pace.map(|mut pace| {
                pace.projected_candidate_burn_basis_points_per_hour = Some(49);
                pace.aggregate_burn_basis_points_per_hour = Some(49);
                pace
            });
        }
        QuotaCaptureDesignCase::StaleUnder => {
            let selected_row = report
                .rows
                .get_mut(0)
                .unwrap_or_else(|| panic!("capture report should include selected row"));
            selected_row.freshness = QuotaEvidenceFreshness::Stale;
            for window in &mut selected_row.windows {
                window.status = QuotaWindowStatus::Stale;
                window.observed_unix_seconds = NOW - 901;
            }
            selected_row.updated = "failed 15m 1s ago: network".to_owned();
            selected_row.weekly_pace = selected_row.weekly_pace.map(|mut pace| {
                pace.projected_candidate_burn_basis_points_per_hour = Some(10);
                pace.aggregate_burn_basis_points_per_hour = Some(10);
                pace
            });
        }
        QuotaCaptureDesignCase::DegradedOver => {
            report.selection_projection_source = SelectionProjectionSource::DisplayWindowsFallback;
            report.preferred_next_account_id = None;
            for row in &mut report.rows {
                row.preferred_next = false;
            }
            let selected_row = report
                .rows
                .get_mut(0)
                .unwrap_or_else(|| panic!("capture report should include selected row"));
            selected_row.weekly_pace = selected_row.weekly_pace.map(|mut pace| {
                pace.projected_candidate_burn_basis_points_per_hour = Some(70);
                pace.aggregate_burn_basis_points_per_hour = Some(70);
                pace
            });
        }
        QuotaCaptureDesignCase::UnavailableBurn => {
            let selected_row = report
                .rows
                .get_mut(0)
                .unwrap_or_else(|| panic!("capture report should include selected row"));
            selected_row.weekly_pace = None;
        }
    }
    report
}

pub(super) fn blocked_quota_capture_report() -> QuotaStatusReport {
    QuotaStatusReport {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        route_band: "responses".to_owned(),
        selected_pool: SelectedPool::None,
        preferred_next_account_id: None,
        selection_projection_source: SelectionProjectionSource::SqlxProjection,
        now_unix_seconds: NOW,
        rows: vec![
            quota_capture_row(QuotaCaptureRowFixture {
                account_id_value: "acct_ssdev",
                account_label: "ssdev",
                preferred_next: false,
                short_remaining: 0,
                weekly_remaining: 0,
                freshness: QuotaEvidenceFreshness::Fresh,
                availability: AccountAvailability::Blocked,
                routing_reason: RoutingReason::BlockedWindowExhausted,
            }),
            quota_capture_row(QuotaCaptureRowFixture {
                account_id_value: "acct_legacy",
                account_label: "legacy",
                preferred_next: false,
                short_remaining: 0,
                weekly_remaining: 0,
                freshness: QuotaEvidenceFreshness::Stale,
                availability: AccountAvailability::Blocked,
                routing_reason: RoutingReason::BlockedWindowExhausted,
            }),
        ],
    }
}

pub(super) fn assert_quota_capture_width_contract(width: usize, text: &str) {
    assert!(
        text.lines().all(|line| line.chars().count() <= width),
        "quota capture width {width} overflowed:\n{text}"
    );
    assert!(
        text.contains('╭') && text.contains('╰') && !text.contains("  Account"),
        "quota capture should render boxed quota blocks:\n{text}"
    );
    if width == 72 {
        for account_label in ["ssdev", "askluna", "matches", "legacy"] {
            assert!(
                text.lines().any(|line| {
                    line.contains(account_label)
                        && line.starts_with('│')
                        && !line.contains("responses ->")
                }),
                "quota capture should include {account_label}:\n{text}"
            );
            assert!(
                text.contains("weekly · resets") && text.contains("5h · resets"),
                "quota capture width 72 should preserve weekly reset facts for {account_label}:\n{text}"
            );
        }
        assert!(
            !text.contains("..."),
            "quota capture width 72 should avoid clipping normal account rows:\n{text}"
        );
    }
    if width == 90 {
        for reason in ["safest quota", "same pool", "5h guard", "quota empty"] {
            assert!(
                text.contains(reason),
                "quota capture width 90 should preserve readable reasons, missing {reason}:\n{text}"
            );
        }
        assert!(
            !text.contains("..."),
            "quota capture width 90 should not clip table cells:\n{text}"
        );
    }
}

pub(super) fn format_next_use_for_capture(reason: RoutingReason) -> &'static str {
    match reason {
        RoutingReason::PreferredNearResetDrainable
        | RoutingReason::PreferredNearResetControlledDrain
        | RoutingReason::PreferredWeeklyHealthier
        | RoutingReason::PreferredWeeklyResetSoon
        | RoutingReason::PreferredShortResetSoon
        | RoutingReason::PreferredProjectedBurn
        | RoutingReason::PreferredSafestQuota
        | RoutingReason::PreferredLastResortShortWindowGuard => "preferred by quota",
        RoutingReason::AvailableSamePool => "available by quota",
        RoutingReason::HeldReserve
        | RoutingReason::HeldUnknown
        | RoutingReason::HeldShortWindowGuard => "held by quota",
        RoutingReason::UnknownFallbackPreferred | RoutingReason::UnknownFallbackAvailable => {
            "fallback by quota"
        }
        RoutingReason::RetiringNearZero => "retiring",
        RoutingReason::ExcludedDisabled
        | RoutingReason::ExcludedMissingCredential
        | RoutingReason::BlockedWindowExhausted
        | RoutingReason::BlockedWindowIneligible => "blocked",
    }
}

pub(super) fn capture_dir() -> PathBuf {
    let dir = std::env::var_os("CODEX_ROUTER_CAPTURE_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp/ux-proof/production"),
        PathBuf::from,
    );
    must_ok(std::fs::create_dir_all(&dir));
    dir
}

pub(super) fn write_capture_pair_with_svg_text(dir: &Path, name: &str, text: &str, svg_text: &str) {
    must_ok(std::fs::write(dir.join(format!("{name}.txt")), text));
    must_ok(std::fs::write(dir.join(format!("{name}.ansi")), svg_text));
    must_ok(std::fs::write(
        dir.join(format!("{name}.svg")),
        terminal_svg(name, svg_text),
    ));
}

pub(super) fn terminal_svg(title: &str, text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let width = lines
        .iter()
        .map(|line| ansi_visible_text(line).chars().count())
        .max()
        .unwrap_or(1);
    let height = lines.len().max(1);
    let pixel_width = width * 9 + 32;
    let pixel_height = height * 18 + 34;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixel_width}\" height=\"{pixel_height}\" viewBox=\"0 0 {pixel_width} {pixel_height}\"><rect width=\"100%\" height=\"100%\" fill=\"#111318\"/>"
    );
    for (index, line) in lines.iter().enumerate() {
        let selected_background = line.contains("\x1b[48;2;58;70;122m");
        if line.contains('*') || line.contains("[blocked]") || selected_background {
            let y = 36 + index * 18;
            let (x, rect_width) = if selected_background {
                (
                    34,
                    ((width.saturating_sub(4) as f64) * 8.4).round() as usize,
                )
            } else {
                (8, pixel_width.saturating_sub(16))
            };
            svg.push_str(&format!(
                "<rect x=\"{x}\" y=\"{}\" width=\"{rect_width}\" height=\"18\" fill=\"#2d333b\"/>",
                y.saturating_sub(14),
            ));
        }
    }
    svg.push_str(&svg_text(16, 24, "#e6edf3", title));
    for (index, line) in lines.iter().enumerate() {
        let y = 44 + index * 18;
        svg.push_str(&svg_line_text(16, y, &ansi_svg_segments(line)));
    }
    svg.push_str("</svg>");
    svg
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SvgTextSegment {
    pub(super) color: &'static str,
    pub(super) text: String,
}

pub(super) fn ansi_visible_text(line: &str) -> String {
    ansi_svg_segments(line)
        .into_iter()
        .map(|segment| segment.text)
        .collect::<String>()
}

pub(super) fn ansi_svg_segments(line: &str) -> Vec<SvgTextSegment> {
    let mut segments = Vec::new();
    let mut current_text = String::new();
    let mut current_color = "#e6edf3";
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\x1b' && characters.peek() == Some(&'[') {
            characters.next();
            let mut sequence = String::new();
            for sequence_character in characters.by_ref() {
                sequence.push(sequence_character);
                if sequence_character.is_ascii_alphabetic() {
                    break;
                }
            }
            if sequence.ends_with('m') {
                if !current_text.is_empty() {
                    segments.push(SvgTextSegment {
                        color: current_color,
                        text: current_text,
                    });
                    current_text = String::new();
                }
                current_color = ansi_sgr_color(&sequence);
            }
            continue;
        }
        current_text.push(character);
    }
    if !current_text.is_empty() {
        segments.push(SvgTextSegment {
            color: current_color,
            text: current_text,
        });
    }
    segments
}

pub(super) fn ansi_sgr_color(sequence: &str) -> &'static str {
    match sequence.trim_end_matches('m') {
        "0" => "#e6edf3",
        "32" => "#7ee787",
        "33" => "#ffe75c",
        "36" => "#8ae8f0",
        "90" => "#8b949e",
        "48;2;58;70;122" => "#e6edf3",
        _ => "#e6edf3",
    }
}

pub(super) fn svg_text(x: usize, y: usize, color: &str, text: &str) -> String {
    format!(
        "<text x=\"{x}\" y=\"{y}\" xml:space=\"preserve\" font-family=\"SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"14\" fill=\"{color}\">{}</text>",
        escape_xml(text)
    )
}

pub(super) fn svg_line_text(x: usize, y: usize, segments: &[SvgTextSegment]) -> String {
    let mut text = format!(
        "<text x=\"{x}\" y=\"{y}\" xml:space=\"preserve\" font-family=\"SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"14\">"
    );
    for segment in segments {
        text.push_str(&format!(
            "<tspan fill=\"{}\">{}</tspan>",
            segment.color,
            escape_xml(&segment.text)
        ));
    }
    text.push_str("</text>");
    text
}

pub(super) fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(super) fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected Ok, got error: {error}"),
    }
}

pub(super) fn account(account_id: &str, label: &str) -> AccountRecord {
    AccountRecord::new(test_account_id(account_id), label, AccountStatus::Enabled)
        .with_active_credential_generation(1)
}

pub(super) fn account_id(value: &str) -> AccountId {
    test_account_id(value)
}

pub(super) fn test_account_id(value: &str) -> AccountId {
    match AccountId::new(value) {
        Ok(account_id) => account_id,
        Err(error) => panic!("test account id is valid: {error}"),
    }
}

pub(super) fn display_window(
    window_seconds: u64,
    remaining_headroom: u32,
    reset_unix_seconds: u64,
    run_rate_estimate: QuotaRunRateEstimate,
) -> DisplayQuotaWindow {
    DisplayQuotaWindow {
        window_seconds,
        status: QuotaWindowStatus::Eligible,
        remaining_headroom,
        reset_unix_seconds: Some(reset_unix_seconds),
        observed_unix_seconds: NOW,
        effective: window_seconds == V1_SHORT_WINDOW_SECONDS,
        run_rate_estimate,
    }
}
