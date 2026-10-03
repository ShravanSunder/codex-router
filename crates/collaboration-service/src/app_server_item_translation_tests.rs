use super::*;
use message_board::SessionRef;
use serde_json::{Value, json};
use session_event_model::{SessionItem, SessionItemKind, StopReason, ToolCallStatus, TurnOutcome};
use uuid::Uuid;

fn replay_session() -> SessionRef {
    serde_json::from_value(json!({
        "endpoint":{
            "serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89",
            "endpointId":"claude-local"
        },
        "sessionId":"provider-session-1"
    }))
    .expect("valid session")
}

fn item(id: &str, kind: SessionItemKind, text: Option<&str>) -> SessionItem {
    SessionItem {
        item_id: id.into(),
        kind,
        text: text.map(str::to_owned),
    }
}

/// Oracle: Codex v2 ThreadItem serde tags in item.rs:236-429 at e72da2b538.
/// An unknown tag would poison the whole thread/read response.
#[test]
fn every_session_item_maps_to_a_known_codex_item() {
    let cases = [
        (SessionItemKind::UserMessage, "userMessage"),
        (SessionItemKind::AgentMessage, "agentMessage"),
        (SessionItemKind::AgentThought, "reasoning"),
        (
            SessionItemKind::ToolCall {
                tool_kind: "execute".into(),
                status: ToolCallStatus::Pending,
            },
            "mcpToolCall",
        ),
        (SessionItemKind::Plan, "plan"),
        (SessionItemKind::Usage, "agentMessage"),
        (SessionItemKind::ModeChange, "agentMessage"),
        (SessionItemKind::ConfigChange, "agentMessage"),
        (SessionItemKind::SessionInfo, "agentMessage"),
        (SessionItemKind::Notice, "agentMessage"),
        (
            SessionItemKind::Unknown {
                source_kind: "vendor".into(),
            },
            "agentMessage",
        ),
    ];
    for (index, (kind, expected_tag)) in cases.into_iter().enumerate() {
        let source = item(&format!("item-{index}"), kind, Some("visible text"));
        let translated = translate_session_item(&source, "thread-1", "turn-1");
        assert_eq!(translated.thread_item["type"], expected_tag);
        assert_eq!(translated.thread_item["id"], source.item_id);
    }
}

/// Oracle: item.rs:323-367 and mcp.rs:208-239. Generic provider tools are
/// represented as mcpToolCall and keep their visible text as result content.
#[test]
fn generic_tool_call_keeps_status_and_text() {
    let source = item(
        "tool-1",
        SessionItemKind::ToolCall {
            tool_kind: "execute".into(),
            status: ToolCallStatus::Completed,
        },
        Some("command output"),
    );
    let translated = translate_session_item(&source, "thread-1", "turn-1");
    assert_eq!(translated.thread_item["type"], "mcpToolCall");
    assert_eq!(translated.thread_item["tool"], "execute");
    assert_eq!(translated.thread_item["status"], "completed");
    assert_eq!(
        translated.thread_item["result"]["content"][0],
        json!({"type":"text","text":"command output"})
    );
}

/// Oracle: v2 turn/plan/updated in turn.rs:568-580. The event model currently
/// has plan text, not structured steps, so no step state is invented.
#[test]
fn plan_emits_a_native_plan_update_without_invented_steps() {
    let source = item(
        "plan-1",
        SessionItemKind::Plan,
        Some("First inspect the API"),
    );
    let translated = translate_session_item(&source, "thread-1", "turn-1");
    assert_eq!(translated.thread_item["type"], "plan");
    assert_eq!(
        translated.notification,
        Some(json!({"method":"turn/plan/updated","params":{
            "threadId":"thread-1","turnId":"turn-1",
            "explanation":"First inspect the API","plan":[]
        }}))
    );
}

/// Oracle: Specification E4; Codex Turn fields in thread_data.rs:386-407.
/// Replay begins a historical Turn on each user message, with an unknown
/// replayed stop reason and a stable UUID alias.
#[test]
fn replay_groups_at_user_messages_without_guessing_steer() {
    let session = replay_session();
    let replay = [
        item("user-1", SessionItemKind::UserMessage, Some("first")),
        item("agent-1", SessionItemKind::AgentMessage, Some("reply")),
        item("user-2", SessionItemKind::UserMessage, Some("second")),
        item("agent-2", SessionItemKind::AgentMessage, Some("reply two")),
    ];
    let turns = group_historical_turns(&session, &replay);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].items.len(), 2);
    assert_eq!(turns[1].items.len(), 2);
    assert_eq!(turns, group_historical_turns(&session, &replay));
    assert!(
        turns
            .iter()
            .all(|turn| Uuid::parse_str(&turn.turn_id).is_ok())
    );
    assert!(turns.iter().all(|turn| matches!(
        &turn.outcome,
        TurnOutcome::Ended {
            stop_reason: StopReason::Unknown(value),
            local_cause: None
        } if value == "replayed"
    )));
    let rendered: Vec<Value> = turns.iter().map(render_historical_turn).collect();
    assert!(rendered.iter().all(|turn| turn["status"] == "completed"));
    assert_eq!(rendered[0]["items"][0]["type"], "userMessage");
}

#[test]
fn replay_preserves_output_before_its_first_user_message() {
    let replay = [
        item("notice", SessionItemKind::Notice, Some("restored session")),
        item("user", SessionItemKind::UserMessage, Some("continue")),
    ];
    let turns = group_historical_turns(&replay_session(), &replay);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].items, replay);
}
