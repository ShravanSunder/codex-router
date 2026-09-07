use super::*;

#[test]
fn conversation_snippets_use_recent_jsonl_user_and_assistant_messages() {
    let jsonl = [
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "please pull main"}]
            }
        })
        .to_string(),
        "not-json".to_owned(),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "checking branch and upstream state"}]
            }
        })
        .to_string(),
        json!({
            "type": "response_item",
            "payload": {
                "type": "tool_call",
                "role": "assistant",
                "content": [{"text": "SECRET_TOOL_OUTPUT"}]
            }
        })
        .to_string(),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "system",
                "content": [{"text": "system content"}]
            }
        })
        .to_string(),
    ]
    .join("\n");

    let snippets = extract_recent_conversation_snippets(&jsonl);

    assert_eq!(
        snippets,
        vec![
            "please pull main".to_owned(),
            "checking branch and upstream state".to_owned()
        ]
    );
}

#[test]
fn conversation_snippets_keep_ten_messages_and_retain_latest_user_context() {
    let mut events = vec![json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "the user request that explains the work"}]
        }
    })];
    events.extend((1..=11).map(|index| {
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": format!("assistant update {index}")}]
            }
        })
    }));
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
#[allow(clippy::expect_used)]
fn conversation_preview_reads_message_beyond_the_previous_tail_limit() {
    let path = std::env::temp_dir().join(format!(
        "codex-router-large-session-history-{}.jsonl",
        std::process::id()
    ));
    let trailing_padding = format!("\n{}", "x\n".repeat(350 * 1024));
    let recent_message = json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "message inside the expanded tail"}]
        }
    })
    .to_string();
    fs::write(&path, format!("{recent_message}{trailing_padding}"))
        .expect("test should write large history fixture");

    let preview = SessionConversationPreview::from_rollout_path(Some(
        path.to_str().expect("temp path should be utf-8"),
    ));
    let _ = fs::remove_file(&path);

    assert_eq!(
        preview.snippets,
        vec!["message inside the expanded tail".to_owned()]
    );
}

#[test]
#[allow(clippy::expect_used)]
fn conversation_preview_recovers_when_tail_starts_inside_utf8_character() {
    let path = std::env::temp_dir().join(format!(
        "codex-router-split-utf8-session-history-{}.jsonl",
        std::process::id()
    ));
    let recent_message = json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "message after split UTF-8 boundary"}]
        }
    })
    .to_string();
    let mut bytes = "épartial line\n".as_bytes().to_vec();
    bytes.extend_from_slice(recent_message.as_bytes());
    bytes.push(b'\n');
    bytes.resize(
        usize::try_from(SESSION_CONVERSATION_MAX_READ_BYTES).expect("tail bound should fit usize")
            + 1,
        b'x',
    );
    fs::write(&path, bytes).expect("test should write split UTF-8 history fixture");

    let preview = SessionConversationPreview::from_rollout_path(Some(
        path.to_str().expect("temp path should be utf-8"),
    ));
    let _ = fs::remove_file(&path);

    assert_eq!(
        preview.snippets,
        vec!["message after split UTF-8 boundary".to_owned()]
    );
}

#[test]
fn conversation_snippets_skip_control_payloads() {
    let jsonl = [
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "# AGENTS.md instructions\n<INSTRUCTIONS>do not display</INSTRUCTIONS>"}]
            }
        })
        .to_string(),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "<hook_prompt hook_run_id=\"x\">control</hook_prompt>"}]
            }
        })
        .to_string(),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "real assistant reply"}]
            }
        })
        .to_string(),
    ]
    .join("\n");

    assert_eq!(
        extract_recent_conversation_snippets(&jsonl),
        vec!["real assistant reply".to_owned()]
    );
}

#[test]
fn conversation_snippets_skip_review_wrapper_prompts() {
    let jsonl = [
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Read-only implementation review. Scope: current uncommitted diff. Review only P0-P2 findings."}]
            }
        })
        .to_string(),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "actual assistant answer"}]
            }
        })
        .to_string(),
    ]
    .join("\n");

    assert_eq!(
        extract_recent_conversation_snippets(&jsonl),
        vec!["actual assistant answer".to_owned()]
    );
}

#[test]
fn conversation_snippets_skip_codex_transcript_wrappers() {
    let jsonl = [
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "The following is the Codex agent history for review. It includes tool call arguments.\n>>> TRANSCRIPT START\nuser: keep the working tree clean"
                }]
            }
        })
        .to_string(),
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "actual resumed thread message"}]
            }
        })
        .to_string(),
    ]
    .join("\n");

    assert_eq!(
        extract_recent_conversation_snippets(&jsonl),
        vec!["actual resumed thread message".to_owned()]
    );
}

#[test]
#[allow(clippy::expect_used)]
fn conversation_preview_reads_rollout_path_with_fallback() {
    let path = std::env::temp_dir().join(format!(
        "codex-router-session-history-{}.jsonl",
        std::process::id()
    ));
    fs::write(
        &path,
        json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "real local history"}]
            }
        })
        .to_string(),
    )
    .expect("test should write history fixture");

    let preview = SessionConversationPreview::from_rollout_path(Some(
        path.to_str().expect("temp path should be utf-8"),
    ));
    let _ = fs::remove_file(&path);

    assert_eq!(preview.snippets, vec!["real local history".to_owned()]);
    assert_eq!(preview.unavailable_reason, None);

    let missing = SessionConversationPreview::from_rollout_path(None);
    assert_eq!(missing.snippets, Vec::<String>::new());
    assert_eq!(
        missing.unavailable_reason,
        Some("history unavailable".to_owned())
    );
}

#[test]
#[allow(clippy::expect_used)]
fn rollout_path_validation_rejects_paths_outside_codex_sessions() {
    let root = std::env::temp_dir().join(format!(
        "codex-router-rollout-validation-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let sessions_dir = codex_home.join("sessions");
    let outside_dir = root.join("outside");
    fs::create_dir_all(&sessions_dir).expect("test should create sessions dir");
    fs::create_dir_all(&outside_dir).expect("test should create outside dir");
    let inside = sessions_dir.join("rollout.jsonl");
    let outside = outside_dir.join("rollout.jsonl");
    fs::write(&inside, "").expect("test should write inside rollout");
    fs::write(&outside, "").expect("test should write outside rollout");

    assert_eq!(
        validated_rollout_path(
            &codex_home,
            Some(inside.to_str().expect("inside path should be utf-8"))
        ),
        Some(
            inside
                .canonicalize()
                .expect("inside path should canonicalize")
                .display()
                .to_string()
        )
    );
    assert_eq!(
        validated_rollout_path(
            &codex_home,
            Some(outside.to_str().expect("outside path should be utf-8"))
        ),
        None
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
#[allow(clippy::expect_used)]
fn deferred_rollout_source_avoids_filesystem_validation_during_session_load() {
    let root = std::env::temp_dir().join(format!(
        "codex-router-deferred-rollout-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let missing_inside = codex_home.join("sessions").join("missing.jsonl");
    let outside = root.join("outside").join("missing.jsonl");

    assert!(
        deferred_rollout_source(
            &codex_home,
            Some(
                missing_inside
                    .to_str()
                    .expect("inside path should be utf-8")
            ),
        )
        .is_some(),
        "startup should keep an inside source candidate without canonicalizing every file"
    );
    assert_eq!(
        deferred_rollout_source(
            &codex_home,
            Some(outside.to_str().expect("outside path should be utf-8")),
        ),
        None,
        "startup should still reject lexically outside rollout paths"
    );
    assert_eq!(
        deferred_rollout_source(&codex_home, Some("../sessions/escape.jsonl")),
        None,
        "startup should reject parent-directory escape paths"
    );
}
