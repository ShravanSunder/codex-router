use super::*;

mod quota_value_formatting;

pub(super) use quota_value_formatting::*;

#[cfg(test)]
pub(super) fn write_quota_table(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
    terminal_width: Option<usize>,
) -> Result<(), QuotaCommandError> {
    write_quota_table_with_style(stdout, report, terminal_width, QuotaTableStyle::PlainText)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QuotaTableStyle {
    #[cfg(test)]
    PlainText,
    TerminalColor,
}

impl QuotaTableStyle {
    pub(super) const fn ansi(self) -> bool {
        match self {
            #[cfg(test)]
            Self::PlainText => false,
            Self::TerminalColor => true,
        }
    }
}

pub(super) fn write_quota_table_with_style(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
    terminal_width: Option<usize>,
    style: QuotaTableStyle,
) -> Result<(), QuotaCommandError> {
    let rows = report.rows();
    let width = terminal_width.unwrap_or(100).max(40);
    let view_model = quota_status_view_model(report, rows, width);
    write_quota_status_view(stdout, view_model, style.ansi()).map_err(QuotaCommandError::Stdout)
}

pub(super) fn write_quota_plain(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
) -> Result<(), QuotaCommandError> {
    let rows = report.rows();
    writeln!(stdout, "codex-router {}", report.app_version).map_err(QuotaCommandError::Stdout)?;
    writeln!(
        stdout,
        "account\tstatus\t5h\tweekly\treset pace\tsample\tupdated\tclients\tresets available\trouting\tnext use"
    )
    .map_err(QuotaCommandError::Stdout)?;
    for row in rows {
        let reset_pace =
            reset_pace_view_model_from_snapshot(row.weekly_pace, report.now_unix_seconds);
        let sample_metadata =
            sample_metadata_from_display_windows(&row.windows, report.now_unix_seconds);
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.account_label,
            row.account_status,
            row.short_window.replace('\n', " "),
            row.weekly_window.replace('\n', " "),
            plain_reset_pace_summary(&reset_pace),
            plain_sample_metadata_summary(&sample_metadata),
            row.updated.replace('\n', " "),
            row.active_clients.replace('\n', " "),
            row.reset_credits_available,
            row.routing.replace('\n', " "),
            row.next_use,
        )
        .map_err(QuotaCommandError::Stdout)?;
    }

    write_selector_summary_plain(stdout, rows)
}

pub(super) fn write_selector_summary_plain(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    writeln!(
        stdout,
        "responses route\tnext: {}\twhy: {}",
        selected_account_label(rows),
        selector_summary(rows)
    )
    .map_err(QuotaCommandError::Stdout)
}

pub(super) fn plain_reset_pace_summary(reset_pace: &ResetPaceViewModel) -> String {
    if reset_pace.state == ResetPaceState::Unavailable {
        return reset_pace.semantic_label.to_owned();
    }
    if let Some(impact_label) = &reset_pace.impact_label {
        return impact_label.clone();
    }
    format!(
        "{} {}",
        reset_pace.multiple_label, reset_pace.semantic_label
    )
}

pub(super) fn plain_sample_metadata_summary(sample_metadata: &SampleMetadata) -> String {
    if sample_metadata.confidence == SampleConfidence::Unknown {
        return sample_metadata.semantic_label.to_owned();
    }
    format!(
        "{} {}",
        sample_metadata.semantic_label, sample_metadata.age_label
    )
}

pub(super) fn selected_account_label(rows: &[QuotaStatusRow]) -> &str {
    rows.iter()
        .find(|row| row.preferred_next)
        .map(|row| row.account_label.as_str())
        .unwrap_or("none")
}

pub(super) fn selector_summary(rows: &[QuotaStatusRow]) -> String {
    let Some(selected_row) = rows.iter().find(|row| row.preferred_next) else {
        return "no usable accounts".to_owned();
    };
    selected_row.routing.replace('\n', " ")
}

pub(super) fn write_quota_json(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
) -> Result<(), QuotaCommandError> {
    let json_report = JsonQuotaStatusReport::from_report(report);
    serde_json::to_writer_pretty(&mut *stdout, &json_report).map_err(|error| {
        QuotaCommandError::Stdout(std::io::Error::other(format!(
            "failed to serialize quota status json: {error}"
        )))
    })?;
    writeln!(stdout).map_err(QuotaCommandError::Stdout)
}
