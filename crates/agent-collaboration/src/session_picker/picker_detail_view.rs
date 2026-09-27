//! Picker detail view.
use super::{
    START_NEW_DETAILS_HEIGHT, SessionConversationPreview, SessionPickerRecord, SessionsPickerModel,
    fit_line, no_matching_sessions_label, start_new_args_label,
};
use iocraft::prelude::*;

pub(super) fn render_start_new_details(
    model: &SessionsPickerModel,
    width: usize,
    height: usize,
) -> AnyElement<'static> {
    let detail_width = width.saturating_sub(4);
    element! {
        View(
            width: width as u32,
            height: height.max(START_NEW_DETAILS_HEIGHT) as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
            overflow: Overflow::Hidden,
        ) {
            Text(content: "Start new session", color: Color::Cyan, weight: Weight::Bold)
            Text(content: fit_line(&start_new_args_label(model), detail_width), color: Color::Yellow, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            #(if model.visible_record_len() == 0 {
                Some(detail_text(no_matching_sessions_label(model), detail_width, Color::Grey))
            } else {
                None
            })
        }
    }
    .into_any()
}

pub(super) fn render_details(
    record: &SessionPickerRecord,
    width: usize,
    selected_conversation: Option<&SessionConversationPreview>,
    height: usize,
) -> AnyElement<'static> {
    if record.identity.is_provider() {
        let detail_width = width.saturating_sub(4);
        let endpoint = record.endpoint_label.as_deref().unwrap_or("Provider");
        let state = match record.provider_state {
            Some(collaboration_client::protocol::ProviderSessionState::Unloaded) => "Unloaded",
            Some(collaboration_client::protocol::ProviderSessionState::Idle) => "Idle",
            Some(collaboration_client::protocol::ProviderSessionState::Running) => "Running",
            Some(collaboration_client::protocol::ProviderSessionState::RequiresAction) => {
                "Requires action"
            }
            Some(collaboration_client::protocol::ProviderSessionState::AuthenticationRequired) => {
                "Authentication required"
            }
            Some(collaboration_client::protocol::ProviderSessionState::Closed) => "Closed",
            None => "Unknown",
        };
        return element! {
            View(
                width: width as u32,
                height: height.max(1) as u32,
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Single,
                border_color: Color::DarkGrey,
                overflow: Overflow::Hidden,
                padding_left: 1,
                padding_right: 1,
            ) {
                Text(content: fit_line(&format!("Session ID: {}", record.session_id), detail_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&format!("Endpoint: {endpoint}"), detail_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&format!("State: {state}"), detail_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&format!("Working directory: {}", record.cwd.as_deref().unwrap_or("-")), detail_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: "Read-only here; provider resume is unavailable.", color: Color::Yellow, wrap: TextWrap::Wrap)
            }
        }
        .into_any();
    }
    let conversation = selected_conversation.unwrap_or(&record.conversation);
    let panel_height = height.max(1);
    let conversation_rows = if conversation.snippets.is_empty() {
        vec![conversation_text(
            conversation
                .unavailable_reason
                .as_deref()
                .unwrap_or("history unavailable"),
            Color::DarkGrey,
        )]
    } else {
        conversation
            .snippets
            .iter()
            .map(|snippet| conversation_text(&format!("• {snippet}"), Color::Grey))
            .collect::<Vec<_>>()
    };

    element! {
        View(
            width: width as u32,
            height: panel_height as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
        ) {
            Text(content: format!("Session ID: {}", record.session_id), color: Color::Grey, weight: Weight::Normal, wrap: TextWrap::NoWrap)
            View(height: 1) { Text(content: "") }
            Text(content: "Conversation", color: Color::Cyan, weight: Weight::Bold)
            #(conversation_rows)
        }
    }
    .into_any()
}

pub(super) fn conversation_text(value: &str, color: Color) -> AnyElement<'static> {
    element! {
        Text(
            content: value.to_owned(),
            color,
            weight: Weight::Normal,
            wrap: TextWrap::Wrap,
        )
    }
    .into_any()
}

pub(super) fn detail_text(value: &str, width: usize, color: Color) -> AnyElement<'static> {
    element! {
        Text(
            content: fit_line(value, width),
            color,
            weight: Weight::Normal,
            wrap: TextWrap::NoWrap,
        )
    }
    .into_any()
}
