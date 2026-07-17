use super::*;

pub(super) async fn render_quota_status(
    stdout: &mut impl Write,
    router_root: PathBuf,
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let effective_format = effective_human_quota_format(format, stdout_is_terminal);
    let unicode_bars = effective_format != QuotaStatusFormat::Plain;
    let report =
        load_quota_status_report_async(&router_root, all_limits, now_unix_seconds, unicode_bars)
            .await?;
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
    reset_session_factory: &dyn crate::quota_reset::InteractiveResetSessionFactory,
) -> Result<(), QuotaCommandError> {
    let width = stdout_terminal_width.unwrap_or(100).max(40);
    let report =
        load_quota_status_report_async(&router_root, all_limits, now_unix_seconds, true).await?;
    let view_model = quota_status_view_model(&report, report.rows(), width);
    let reload_view_model =
        quota_status_view_model_loader(router_root.clone(), all_limits, true, width);
    let reset_session = reset_session_factory.create(&router_root)?;
    let shutdown_sender = reset_session.ports.intent_sender.clone();
    let session_task = tokio::spawn(reset_session.runner);
    let render_result = run_quota_status_view(
        view_model,
        Some(reload_view_model),
        Some(reset_session.ports),
    )
    .await;
    let _ = shutdown_sender
        .send(crate::quota_reset::reset_session_supervisor::ResetSessionIntent::Shutdown)
        .await;
    drop(shutdown_sender);
    let session_outcome = await_reset_session_task(session_task).await?;
    match session_outcome {
        crate::quota_reset::reset_session_supervisor::ResetSessionOutcome::Cancelled
        | crate::quota_reset::reset_session_supervisor::ResetSessionOutcome::Finished(_) => {}
    }
    render_result.map_err(QuotaCommandError::Stdout)
}

pub(super) async fn await_reset_session_task(
    session_task: tokio::task::JoinHandle<
        crate::quota_reset::reset_session_supervisor::ResetSessionOutcome,
    >,
) -> Result<crate::quota_reset::reset_session_supervisor::ResetSessionOutcome, QuotaCommandError> {
    session_task
        .await
        .map_err(|_join_error| QuotaCommandError::ResetSessionTaskFailed)
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
