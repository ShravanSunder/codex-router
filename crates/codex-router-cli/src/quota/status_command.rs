use super::*;

pub(super) fn render_quota_status(
    stdout: &mut impl Write,
    router_root: PathBuf,
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    render_quota_status_once(
        stdout,
        &router_root,
        format,
        stdout_is_terminal,
        stdout_terminal_width,
        all_limits,
        now_unix_seconds,
    )
}

pub(super) fn render_quota_status_once(
    stdout: &mut impl Write,
    router_root: &Path,
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let effective_format = effective_human_quota_format(format, stdout_is_terminal);
    let unicode_bars = effective_format != QuotaStatusFormat::Plain;
    let report = load_quota_status_report(router_root, all_limits, now_unix_seconds, unicode_bars)?;
    match effective_format {
        QuotaStatusFormat::Table => write_quota_table_with_style(
            stdout,
            &report,
            stdout_terminal_width,
            QuotaTableStyle::TerminalColor,
        ),
        QuotaStatusFormat::Plain => write_quota_plain(stdout, &report),
        QuotaStatusFormat::Json => write_quota_json(stdout, &report),
    }
}

pub(super) async fn render_interactive_quota_status(
    router_root: PathBuf,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let width = stdout_terminal_width.unwrap_or(100).max(40);
    let report =
        load_quota_status_report_async(&router_root, all_limits, now_unix_seconds, true).await?;
    let view_model = quota_status_view_model(&report, report.rows(), width);
    let reload_view_model = quota_status_view_model_loader(router_root, all_limits, true, width);
    run_quota_status_view(view_model, Some(reload_view_model))
        .await
        .map_err(QuotaCommandError::Stdout)
}

pub(super) fn quota_status_view_model_loader(
    router_root: PathBuf,
    all_limits: bool,
    unicode_bars: bool,
    width: usize,
) -> QuotaStatusViewModelLoader {
    Arc::new(move || {
        let router_root = router_root.clone();
        Box::pin(async move {
            let report = load_quota_status_report_async(
                &router_root,
                all_limits,
                current_unix_seconds(),
                unicode_bars,
            )
            .await
            .ok()?;
            Some(quota_status_view_model(&report, report.rows(), width))
        })
    })
}

pub(super) fn effective_human_quota_format(
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
) -> QuotaStatusFormat {
    match format {
        QuotaStatusFormat::Json | QuotaStatusFormat::Plain => format,
        QuotaStatusFormat::Table if stdout_is_terminal => QuotaStatusFormat::Table,
        QuotaStatusFormat::Table => QuotaStatusFormat::Plain,
    }
}
