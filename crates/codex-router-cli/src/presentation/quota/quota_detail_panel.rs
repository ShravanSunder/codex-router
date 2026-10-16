use super::quota_account_options::*;
use super::quota_browse_rendering::*;
use super::quota_floor_editor::*;
use super::quota_reset_detail_rendering::*;
use super::quota_reset_presentation_model::*;
use super::quota_status_view_model::*;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use iocraft::prelude::*;

pub(super) struct QuotaDetailPanelProps<'a> {
    pub(super) interactive: bool,
    pub(super) focused_details: Option<&'a QuotaSelectedAccountViewModel>,
    pub(super) account_options: Option<&'a AccountOptionsState>,
    pub(super) reset_snapshot: Option<&'a ResetWorkflowSnapshot>,
    pub(super) reset_target: Option<&'a ResetPaneTarget>,
    pub(super) weekly_floor_editor: Option<&'a WeeklyFloorEditorState>,
    pub(super) weekly_floor_editor_state: State<Option<WeeklyFloorEditorState>>,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) inventory_page_start: usize,
    pub(super) spinner_tick: usize,
}

pub(super) fn render_detail_panel(props: QuotaDetailPanelProps<'_>) -> AnyElement<'static> {
    if let Some(options) = props.account_options {
        return render_account_options_panel(AccountOptionsPanelProps {
            options,
            reset_snapshot: props.reset_snapshot,
            reset_target: props.reset_target,
            width: props.width,
            height: props.height,
            inventory_page_start: props.inventory_page_start,
            spinner_tick: props.spinner_tick,
        });
    }
    if let Some(editor) = props.weekly_floor_editor {
        return render_weekly_floor_editor(
            editor,
            props.width,
            props.height,
            props.weekly_floor_editor_state,
        );
    }
    if let (Some(snapshot), Some(target)) = (props.reset_snapshot, props.reset_target)
        && reset_mode(Some(snapshot))
    {
        return render_reset_panel(
            snapshot,
            target,
            props.width,
            props.height,
            props.inventory_page_start,
            reset_inventory_page_size(props.height),
            props.spinner_tick,
        );
    }
    render_selected_panel(
        props.focused_details,
        props.width,
        props.height,
        props.interactive,
    )
}
