use codex_acp_adapter::{AcpSchemaCatalog, PromptTarget, project_assistant_text};
use serde_json::json;

#[test]
fn assistant_updates_preserve_text_and_exclude_other_threads_and_turns() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
    let message = |thread, turn| json!({"method":"item/agentMessage/delta","params":{"threadId":thread,"turnId":turn,"itemId":"message-1","delta":"hello\n世界"}});
    for (thread, turn, included) in [
        ("a", "turn-a", true),
        ("b", "turn-a", false),
        ("a", "turn-b", false),
    ] {
        let projected = project_assistant_text(
            &mut catalog,
            PromptTarget {
                session_id: "a",
                turn_id: "turn-a",
            },
            &message(thread, turn),
        )
        .unwrap_or_else(|error| panic!("projection: {error}"));
        assert_eq!(projected.is_some(), included);
        if let Some(projected) = projected {
            assert_eq!(projected["method"], "session/update");
            assert_eq!(
                projected["params"]["update"]["content"]["text"],
                "hello\n世界"
            );
            assert!(
                catalog
                    .validate("SessionNotification", &projected["params"])
                    .unwrap_or_else(|error| panic!("schema: {error}"))
            );
        }
    }
}
