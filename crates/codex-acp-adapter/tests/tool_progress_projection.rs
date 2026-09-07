use codex_acp_adapter::{AcpSchemaCatalog, PromptTarget, project_tool_progress};
use serde_json::json;

#[test]
fn tool_and_plan_updates_keep_status_output_and_scope() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
    for (method, status, update_kind) in [
        ("item/started", "inProgress", "tool_call"),
        ("item/completed", "failed", "tool_call_update"),
    ] {
        let message = json!({"method":method,"params":{"threadId":"thread","turnId":"turn","item":{"type":"commandExecution","id":"tool","command":"cargo test","status":status,"aggregatedOutput":"test result"}}});
        let update = project_tool_progress(
            &mut catalog,
            PromptTarget {
                session_id: "thread",
                turn_id: "turn",
            },
            &message,
        )
        .unwrap_or_else(|error| panic!("projection: {error}"))
        .unwrap_or_else(|| panic!("update"));
        assert_eq!(update["params"]["update"]["sessionUpdate"], update_kind);
        assert_eq!(
            update["params"]["update"]["content"][0]["content"]["text"],
            "test result"
        );
        assert_eq!(
            update["params"]["update"]["status"],
            if status == "failed" {
                "failed"
            } else {
                "in_progress"
            }
        );
        assert!(
            project_tool_progress(
                &mut catalog,
                PromptTarget {
                    session_id: "other",
                    turn_id: "turn"
                },
                &message
            )
            .unwrap_or_else(|error| panic!("scope: {error}"))
            .is_none()
        );
    }
    let plan = json!({"method":"turn/plan/updated","params":{"threadId":"thread","turnId":"turn","plan":[{"step":"Run tests","status":"inProgress"}]}});
    let update = project_tool_progress(
        &mut catalog,
        PromptTarget {
            session_id: "thread",
            turn_id: "turn",
        },
        &plan,
    )
    .unwrap_or_else(|error| panic!("plan: {error}"))
    .unwrap_or_else(|| panic!("plan update"));
    assert_eq!(
        update["params"]["update"]["entries"][0]["status"],
        "in_progress"
    );
}

#[test]
fn file_updates_keep_native_diff_and_unknown_status_is_rejected() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
    let mut message = json!({"method":"item/completed","params":{"threadId":"thread","turnId":"turn","item":{"type":"fileChange","id":"file-change","status":"completed","changes":[{"path":"/work/main.rs","kind":{"type":"update","move_path":null},"diff":"-old\n+new"}]}}});
    let update = project_tool_progress(
        &mut catalog,
        PromptTarget {
            session_id: "thread",
            turn_id: "turn",
        },
        &message,
    )
    .unwrap_or_else(|error| panic!("projection: {error}"))
    .unwrap_or_else(|| panic!("update"));
    assert_eq!(
        update["params"]["update"]["locations"][0]["path"],
        "/work/main.rs"
    );
    assert!(
        update["params"]["update"]["content"][0]["content"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("-old\n+new"))
    );
    message["params"]["item"]["status"] = json!("unknown-success");
    assert!(
        project_tool_progress(
            &mut catalog,
            PromptTarget {
                session_id: "thread",
                turn_id: "turn"
            },
            &message
        )
        .is_err()
    );
}
