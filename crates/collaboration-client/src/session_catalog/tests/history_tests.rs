use super::*;
use serde_json::json;
use std::fs;

#[test]
fn conversation_snippets_keep_ten_messages_and_retain_latest_user_context() {
    let mut events = vec![json!({
        "type": "response_item",
        "payload": {"type": "message", "role": "user", "content": [{"text": "the user request that explains the work"}]}
    })];
    events.extend((1..=11).map(|index| json!({
        "type": "response_item",
        "payload": {"type": "message", "role": "assistant", "content": [{"text": format!("assistant update {index}")}]}
    })));
    let jsonl = events
        .into_iter()
        .map(|event| event.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let snippets = extract_recent_conversation_snippets(&jsonl);

    assert_eq!(snippets.len(), 10);
    assert_eq!(
        snippets.first().map(String::as_str),
        Some("the user request that explains the work")
    );
    assert_eq!(
        snippets.get(1).map(String::as_str),
        Some("assistant update 3")
    );
    assert_eq!(
        snippets.last().map(String::as_str),
        Some("assistant update 11")
    );
}

#[test]
fn conversation_snippets_use_recent_jsonl_user_and_assistant_messages() {
    let jsonl = [
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"please pull main"}]}}).to_string(),
        "not-json".to_owned(),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"checking branch and upstream state"}]}}).to_string(),
        json!({"type":"response_item","payload":{"type":"tool_call","role":"assistant","content":[{"text":"SECRET_TOOL_OUTPUT"}]}}).to_string(),
        json!({"type":"response_item","payload":{"type":"message","role":"system","content":[{"text":"system content"}]}}).to_string(),
    ]
    .join("\n");

    assert_eq!(
        extract_recent_conversation_snippets(&jsonl),
        vec![
            "please pull main".to_owned(),
            "checking branch and upstream state".to_owned()
        ]
    );
}

#[test]
fn conversation_snippets_skip_control_payloads() {
    let jsonl = [
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"# AGENTS.md instructions\n<INSTRUCTIONS>do not display</INSTRUCTIONS>"}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"<hook_prompt hook_run_id=\"x\">control</hook_prompt>"}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"real assistant reply"}]}}),
    ].into_iter().map(|event| event.to_string()).collect::<Vec<_>>().join("\n");
    assert_eq!(
        extract_recent_conversation_snippets(&jsonl),
        ["real assistant reply"]
    );
}

#[test]
fn conversation_snippets_skip_review_wrapper_prompts() {
    let jsonl = [
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"Read-only implementation review. Scope: current uncommitted diff. Review only P0-P2 findings."}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"actual assistant answer"}]}}),
    ].into_iter().map(|event| event.to_string()).collect::<Vec<_>>().join("\n");
    assert_eq!(
        extract_recent_conversation_snippets(&jsonl),
        ["actual assistant answer"]
    );
}

#[test]
fn conversation_snippets_skip_codex_transcript_wrappers() {
    let jsonl = [
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"The following is the Codex agent history for review. It includes tool call arguments.\n>>> TRANSCRIPT START\nuser: keep the working tree clean"}]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"actual resumed thread message"}]}}),
    ].into_iter().map(|event| event.to_string()).collect::<Vec<_>>().join("\n");
    assert_eq!(
        extract_recent_conversation_snippets(&jsonl),
        ["actual resumed thread message"]
    );
}

#[test]
fn conversation_preview_reads_rollout_path_with_fallback() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-session-history-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    let path = sessions.join("rollout.jsonl");
    fs::write(
        &path,
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"real local history"}]}}).to_string(),
    )
    .expect("write history fixture");
    let source = SessionHistorySource::from_catalog_path(&codex_home, path.to_str())
        .expect("inside rollout source");

    let history = read_session_conversation_history(Some(&source));
    assert_eq!(history.snippets, ["real local history"]);
    assert_eq!(history.unavailable_reason, None);

    let missing = read_session_conversation_history(None);
    assert!(missing.snippets.is_empty());
    assert_eq!(
        missing.unavailable_reason.as_deref(),
        Some("history unavailable")
    );
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn deferred_rollout_source_avoids_filesystem_validation_during_session_load() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-history-source-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let inside = codex_home.join("sessions/missing.jsonl");
    let outside = root.join("outside/missing.jsonl");

    assert!(SessionHistorySource::from_catalog_path(&codex_home, inside.to_str()).is_some());
    assert!(SessionHistorySource::from_catalog_path(&codex_home, outside.to_str()).is_none());
    assert!(
        SessionHistorySource::from_catalog_path(&codex_home, Some("../sessions/escape.jsonl"))
            .is_none()
    );
}

#[test]
fn rollout_path_validation_rejects_paths_outside_codex_sessions() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-history-containment-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let sessions = codex_home.join("sessions");
    let outside = root.join("outside");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::create_dir_all(&outside).expect("create outside");
    let inside_path = sessions.join("rollout.jsonl");
    let outside_path = outside.join("rollout.jsonl");
    let event = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"trusted history"}]}}).to_string();
    fs::write(&inside_path, &event).expect("write inside");
    fs::write(&outside_path, &event).expect("write outside");

    let inside = SessionHistorySource::new(inside_path.display().to_string(), codex_home.clone());
    let outside = SessionHistorySource::new(outside_path.display().to_string(), codex_home);
    assert_eq!(
        read_session_conversation_history(Some(&inside)).snippets,
        ["trusted history"]
    );
    assert_eq!(
        read_session_conversation_history(Some(&outside))
            .unavailable_reason
            .as_deref(),
        Some("history unavailable")
    );
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn conversation_preview_recovers_when_tail_starts_inside_utf8_character() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-history-tail-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    let path = sessions.join("rollout.jsonl");
    let recent = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"message after split UTF-8 boundary"}]}}).to_string();
    let mut bytes = "épartial line\n".as_bytes().to_vec();
    bytes.extend_from_slice(recent.as_bytes());
    bytes.push(b'\n');
    bytes.resize(SESSION_CONVERSATION_MAX_READ_BYTES as usize + 1, b'x');
    fs::write(&path, bytes).expect("write history");

    let source = SessionHistorySource::new(path.display().to_string(), codex_home);
    assert_eq!(
        read_session_conversation_history(Some(&source)).snippets,
        ["message after split UTF-8 boundary"]
    );
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn conversation_preview_reads_message_beyond_the_previous_tail_limit() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-expanded-history-tail-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    let path = sessions.join("rollout.jsonl");
    let recent = json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{"text": "message inside the expanded tail"}]
        }
    })
    .to_string();
    let trailing_padding = format!("\n{}", "x\n".repeat(350 * 1024));
    fs::write(&path, format!("{recent}{trailing_padding}")).expect("write large history");

    let source = SessionHistorySource::new(path.display().to_string(), codex_home);
    assert_eq!(
        read_session_conversation_history(Some(&source)).snippets,
        ["message inside the expanded tail"]
    );
    fs::remove_dir_all(root).expect("remove fixture");
}
