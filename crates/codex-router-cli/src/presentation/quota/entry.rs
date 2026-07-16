use std::io;
use std::io::Write;

use iocraft::prelude::*;

use crate::quota_reset::supervisor::ResetSessionPorts;

use super::component::LIVE_QUOTA_STATUS_RELOAD_INTERVAL;
use super::component::LIVE_QUOTA_STATUS_SPINNER_INTERVAL;
use super::component::MIN_QUOTA_WIDTH;
use super::component::NARROW_QUOTA_WIDTH;
use super::component::QuotaStatusComponent;
use super::component::SIDECAR_QUOTA_WIDTH;
use super::layout::quota_account_list_height;
use super::layout::selected_detail_height;
use super::model::QuotaStatusViewModel;
use super::model::QuotaStatusViewModelLoader;
use super::render::colorize_reset_pace_ansi;

pub(crate) fn write_quota_status_view(
    writer: &mut impl Write,
    view_model: QuotaStatusViewModel,
    ansi: bool,
) -> io::Result<()> {
    let width = view_model.width.max(MIN_QUOTA_WIDTH);
    let height = quota_static_render_height(&view_model, width);
    let mut element = element! {
        QuotaStatusComponent(view_model: view_model, width: width, height: height)
    };
    let canvas = element.render(None);
    if ansi {
        let mut output = Vec::new();
        canvas.write(&mut output)?;
        let text = String::from_utf8(output)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        writer.write_all(colorize_reset_pace_ansi(&text).as_bytes())
    } else {
        canvas.write(writer)
    }
}

pub(super) fn quota_static_render_height(view_model: &QuotaStatusViewModel, width: usize) -> usize {
    let row_count = view_model.rows.len();
    let focused_row_index = view_model
        .rows
        .iter()
        .position(|row| row.selected)
        .or_else(|| (row_count > 0).then_some(0));
    let list_height = quota_account_list_height(row_count, focused_row_index, row_count);
    let details_height =
        selected_detail_height(focused_row_index.is_some() || view_model.selected.is_some());
    let body_height = if width >= SIDECAR_QUOTA_WIDTH {
        list_height.max(details_height)
    } else if width >= NARROW_QUOTA_WIDTH {
        list_height + details_height
    } else {
        list_height
    };
    5 + body_height
}

pub(crate) async fn run_quota_status_view(
    view_model: QuotaStatusViewModel,
    reload_view_model: Option<QuotaStatusViewModelLoader>,
    reset_session_ports: Option<ResetSessionPorts>,
) -> io::Result<()> {
    let (reset_intent_sender, reset_snapshot_receiver) = reset_session_ports
        .map(|ports| (Some(ports.intent_sender), Some(ports.snapshot_receiver)))
        .unwrap_or((None, None));
    element! {
        QuotaStatusComponent(
            view_model: view_model,
            width: 0usize,
            height: 0usize,
            reload_view_model,
            reset_intent_sender,
            reset_snapshot_receiver,
            reload_interval: LIVE_QUOTA_STATUS_RELOAD_INTERVAL,
            spinner_interval: LIVE_QUOTA_STATUS_SPINNER_INTERVAL,
        )
    }
    .render_loop()
    .ignore_ctrl_c()
    .await
}
