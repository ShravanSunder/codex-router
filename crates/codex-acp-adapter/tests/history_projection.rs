use codex_acp_adapter::{AcpSchemaCatalog, project_history};
use serde_json::json;

#[test]
fn supported_history_preserves_user_assistant_order_and_exact_session() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
    let history = json!({"thread":{"id":"thread-a","turns":[{"id":"turn-1","items":[
        {"type":"userMessage","id":"u1","content":[{"type":"text","text":"question"}]},
        {"type":"agentMessage","id":"a1","text":"answer"}
    ]},{"id":"turn-2","items":[{"type":"userMessage","id":"u2","content":[{"type":"text","text":"next"}]}]}]}});
    let updates = project_history(&mut catalog, "thread-a", &history)
        .unwrap_or_else(|error| panic!("project: {error}"));
    assert_eq!(updates.len(), 3);
    assert_eq!(
        updates[0]["params"]["update"]["sessionUpdate"],
        "user_message_chunk"
    );
    assert_eq!(
        updates[1]["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(updates[2]["params"]["update"]["content"]["text"], "next");
    assert!(project_history(&mut catalog, "other-thread", &history).is_err());
    assert!(
        project_history(
            &mut catalog,
            "thread-a",
            &json!({"thread":{"id":"thread-a"}})
        )
        .is_err()
    );
}
