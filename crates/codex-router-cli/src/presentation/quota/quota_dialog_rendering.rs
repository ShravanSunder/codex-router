//! Quota content composed into the shared dialog; effects stay with the owners.
use super::quota_browse_rendering::render_selected_details;
use super::quota_detail_panel::*;
use super::quota_provider_filter_rendering::*;
use super::quota_provider_filters::*;
use super::quota_status_view_model::QuotaStatusViewModel;
use codex_router_core::ids::AccountId;
use codex_router_tui::{DialogAction, DialogFrame, DialogMargins, DialogPresentation};
use iocraft::prelude::*;

pub(super) struct QuotaDialogViewProps<'a> {
    pub width: u16,
    pub height: u16,
    pub content_width: usize,
    pub filter_state: QuotaFilterState,
    pub filter_controller: QuotaFilterController,
    pub details_dialog: State<Option<AccountId>>,
    pub view_model: &'a QuotaStatusViewModel,
    pub sidecar: bool,
    pub filter_modal_owns_input: bool,
    pub help_action: Option<AnyElement<'static>>,
    pub actions: Vec<AnyElement<'static>>,
    pub panel: QuotaDetailPanelProps<'a>,
}

/// The options panel and its page-key handler share the same visible capacity.
pub(super) fn quota_dialog_panel_height(viewport_height: usize, content_height: usize) -> usize {
    let body_height = if viewport_height < 28 {
        viewport_height.saturating_sub(4)
    } else {
        viewport_height.saturating_sub(2).min(28).saturating_sub(6)
    };
    content_height.min(body_height)
}

pub(super) fn render_quota_dialog(props: QuotaDialogViewProps<'_>) -> Option<AnyElement<'static>> {
    let QuotaDialogViewProps {
        width,
        height,
        content_width,
        filter_state,
        filter_controller,
        mut details_dialog,
        view_model,
        sidecar,
        filter_modal_owns_input,
        help_action,
        actions,
        mut panel,
    } = props;
    let short_editor = filter_modal_owns_input && height < 28;
    let presentation = if width < 90 || short_editor {
        DialogPresentation::FullScreen
    } else {
        DialogPresentation::Framed {
            maximum_width: 100,
            maximum_height: 28,
        }
    };
    let margins = if short_editor {
        DialogMargins::default()
    } else {
        DialogMargins {
            top: 1,
            right: 2,
            bottom: 1,
            left: 2,
        }
    };
    if let Some(draft) = filter_state.draft.filter(|_| !filter_modal_owns_input) {
        return Some(element! {
            DialogFrame(width,height,title:"Account filters".to_owned(),help_action,
                presentation:if width<90 {DialogPresentation::FullScreen} else {DialogPresentation::Framed{maximum_width:72,maximum_height:16}},
                margins,
                actions:vec![render_quota_filter_actions(draft,filter_controller)]) {
                #(render_quota_filter_chooser(draft,content_width.min(64).min(usize::from(width).saturating_sub(8)),filter_controller))
            }
        }.into_any());
    }
    if details_dialog.read().is_none() && (sidecar || !filter_modal_owns_input) {
        return None;
    }
    if short_editor {
        panel.width = usize::from(width).saturating_sub(2);
    }
    panel.height = quota_dialog_panel_height(usize::from(height), panel.height);
    if let Some(identity) = details_dialog.read().as_ref() {
        panel.focused_details = view_model
            .rows
            .iter()
            .find(|row| &row.account_id == identity)
            .map(|row| &row.details);
    }
    let frame_width = match presentation {
        DialogPresentation::FullScreen => width.saturating_sub(margins.left + margins.right),
        DialogPresentation::Framed { maximum_width, .. } => {
            maximum_width.min(width.saturating_sub(margins.left + margins.right))
        }
    };
    let content = if filter_modal_owns_input {
        render_detail_panel(panel)
    } else {
        // Keep content natural-sized: the shared ScrollView owns clipping and
        // scrolling, rather than receiving a panel clipped before it can scroll.
        panel
            .focused_details
            .map(|details| {
                render_selected_details(details, usize::from(frame_width).saturating_sub(4), true)
            })
            .unwrap_or_else(|| {
                element!(Text(content:"No selectable account",color:Color::Grey)).into_any()
            })
    };
    Some(element! {
        DialogFrame(width,height,title:"Account details".to_owned(),presentation,margins,help_action,
            padding: if short_editor { 0u16 } else { 1u16 },
            keyboard_scroll:!filter_modal_owns_input,
            actions:if filter_modal_owns_input {
                actions
            } else {
                vec![element!(DialogAction(label:"Close",handler:move |_|details_dialog.set(None))).into_any()]
            }) {
            #(content)
        }
    }.into_any())
}
