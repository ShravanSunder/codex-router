use super::*;

#[test]
fn quota_status_width_contract_preserves_layout() {
    let report = quota_capture_report();

    for width in [48, 72, 90, 120] {
        let mut output = Vec::new();
        must_ok(write_quota_table(&mut output, &report, Some(width)));
        let text = must_ok(String::from_utf8(output));
        assert_quota_capture_width_contract(width, &text);
    }

    let blocked_report = blocked_quota_capture_report();
    let mut output = Vec::new();
    must_ok(write_quota_table(&mut output, &blocked_report, Some(80)));
    let text = must_ok(String::from_utf8(output));
    assert!(
        text.contains("responses -> none    no usable accounts"),
        "blocked capture should expose compact no-selection route state:\n{text}"
    );
    assert!(
        text.lines().all(|line| line.chars().count() <= 80),
        "blocked quota capture overflowed:\n{text}"
    );
}

#[test]
fn quota_status_empty_windows_keep_weekly_bar_and_show_exhausted_reset_pace() {
    let mut report = blocked_quota_capture_report();
    for row in &mut report.rows {
        for window in &mut row.windows {
            window.status = QuotaWindowStatus::Ineligible;
        }
    }
    let mut output = Vec::new();

    must_ok(write_quota_table(&mut output, &report, Some(120)));
    let text = must_ok(String::from_utf8(output));

    assert!(
        text.contains("░░░░░░░░░░ 0% left, reset 7d"),
        "depleted weekly quota should keep its quota bar and reset hint:\n{text}"
    );
    assert!(text.contains("Exhausted"), "{text}");
    assert!(
        !text.contains("🅇  Exhausted"),
        "depleted reset pace should not include the icon marker:\n{text}"
    );
    assert!(
        !text.contains("runs out now"),
        "depleted reset pace should not show old runout copy:\n{text}"
    );
}

#[test]
fn quota_status_terminal_color_keeps_exhausted_red() {
    let report = blocked_quota_capture_report();
    let mut output = Vec::new();

    must_ok(write_quota_table_with_style(
        &mut output,
        &report,
        Some(120),
        QuotaTableStyle::TerminalColor,
    ));
    let text = must_ok(String::from_utf8(output));

    assert!(
        text.contains("\u{1b}[38;5;9mExhausted"),
        "exhausted quota label should keep the red over-burning color:\n{text:?}"
    );
}

#[test]
fn quota_status_table_separates_quota_bars_from_burn_bars() {
    let report = quota_capture_report();
    let mut output = Vec::new();

    must_ok(write_quota_table(&mut output, &report, Some(120)));
    let text = must_ok(String::from_utf8(output));

    assert!(
        !text.contains("  Account") && !text.contains("Status") && !text.contains("Pace"),
        "account list should not render table headers:\n{text}"
    );
    assert!(
        text.contains("Quota windows") && text.contains("Reset pace"),
        "selected account details should separate quota windows from reset pace:\n{text}"
    );
    assert!(
        text.contains("%/h") && text.contains("%/h/conn"),
        "quota table should expose total and per-connection rate units:\n{text}"
    );
    assert!(
        text.contains("weekly · resets")
            && text.contains("5h · resets")
            && text.contains("█")
            && text.contains("%"),
        "main account rows should show weekly and 5h quota lines after connection/reset metadata:\n{text}"
    );
    assert!(
        text.contains("weekly pace") || text.contains("weekly · runs out"),
        "account list should end with weekly burndown:\n{text}"
    );
    assert!(
        text.contains("Reset pace"),
        "selected details should retain reset pace diagnostics:\n{text}"
    );
    assert!(
        !text.contains("current [")
            && !text.contains("safe pace")
            && !text.contains("ahead to reset")
            && !text.contains("safe pace unknown"),
        "quota table should not use legacy burn/safe-pace copy:\n{text}"
    );
}

#[test]
fn quota_status_table_shows_stale_values_with_sample_marker_without_refresh_filler() {
    let mut report = quota_capture_report();
    let row = report
        .rows
        .get_mut(0)
        .unwrap_or_else(|| panic!("capture report should include a selected row"));
    for window in &mut row.windows {
        window.status = QuotaWindowStatus::Stale;
        window.observed_unix_seconds = NOW - 901;
    }
    row.short_window = format_window_cell(&row.windows, V1_SHORT_WINDOW_SECONDS, NOW, true);
    row.weekly_window = format_window_cell(&row.windows, V1_WEEKLY_WINDOW_SECONDS, NOW, true);
    row.freshness = QuotaEvidenceFreshness::Stale;

    let mut output = Vec::new();
    must_ok(write_quota_table(&mut output, &report, Some(120)));
    let text = must_ok(String::from_utf8(output));

    assert!(text.contains("█") && text.contains("% left"), "{text}");
    assert!(text.contains("sample stale 15m 1s"), "{text}");
    assert!(
        !text.contains("needs refresh"),
        "stale value-bearing status output should show values and mark sample stale once:\n{text}"
    );
}

#[test]
fn quota_status_view_model_route_line_compacts_reason_and_burn_rate() {
    let report = quota_capture_report();
    let view_model = quota_status_view_model(&report, report.rows(), 120);

    assert_eq!(
        view_model.route_line, "responses -> ssdev    safest quota    burn 0.1%/h",
        "route line should identify the selected account, reason, burn rate, and limiting window without a second header line"
    );
    assert!(view_model.why_line.is_empty());
}

#[test]
fn quota_status_view_model_reports_serving_clients_from_active_mirror() {
    let report = quota_capture_report();
    let view_model = quota_status_view_model(&report, report.rows(), 120);

    assert_eq!(view_model.serving_clients, Some(5));
}

#[test]
fn quota_status_table_can_emit_terminal_color() {
    let report = quota_capture_report();
    let mut output = Vec::new();

    must_ok(write_quota_table_with_style(
        &mut output,
        &report,
        Some(120),
        QuotaTableStyle::TerminalColor,
    ));
    let text = must_ok(String::from_utf8(output));

    assert!(
        text.contains("\x1b["),
        "quota table should emit ANSI styling:\n{text:?}"
    );
    assert!(
        text.contains("\x1b[38;5;11m") && text.contains("pace under"),
        "quota pace should emit state color:\n{text:?}"
    );
    assert!(
        !text.contains("\x1b[32m"),
        "quota status should avoid the old mixed green/yellow status palette:\n{text:?}"
    );
    assert!(
        !text.contains("\x1b[48;2;58;70;122m"),
        "quota colors should not use the old blue selected-row background:\n{text:?}"
    );
}

#[test]
#[ignore = "writes visual quota capture artifacts for design review"]
fn quota_status_capture_artifacts_for_design_review() {
    let capture_dir = capture_dir();

    for case in QuotaCaptureDesignCase::ALL {
        let report = quota_capture_case_report(case);
        for width in [48, 160] {
            let mut output = Vec::new();
            must_ok(write_quota_table(&mut output, &report, Some(width)));
            let text = must_ok(String::from_utf8(output));
            let mut ansi_output = Vec::new();
            must_ok(write_quota_table_with_style(
                &mut ansi_output,
                &report,
                Some(width),
                QuotaTableStyle::TerminalColor,
            ));
            let ansi_text = must_ok(String::from_utf8(ansi_output));
            write_capture_pair_with_svg_text(
                &capture_dir,
                &format!("{}-{width}", case.file_stem()),
                &text,
                &ansi_text,
            );
        }
    }
}
