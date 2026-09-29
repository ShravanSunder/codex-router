use super::*;

#[test]
fn explicit_session_name_precedes_derived_title_and_is_searchable() {
    let mut record = search_consistency_record(None, None);
    record.name = Some("Website stuff".to_owned());
    record.title = Some("Derived first-message title".to_owned());

    let picker_record = SessionPickerRecord::from_record(&record);

    assert_eq!(
        picker_record.title,
        "Website stuff | Derived first-message title"
    );
    for query in ["website stuff", "derived first-message"] {
        let expression = SessionSearchExpression::parse(query);
        assert!(record.matches_search(&expression), "loader search: {query}");
        assert!(
            picker_record.matches_search(&expression),
            "picker search: {query}"
        );
    }
}

#[test]
fn missing_explicit_session_name_keeps_the_previous_display_fallback() {
    let mut record = search_consistency_record(None, Some("fallback first message"));
    record.title = Some("previous title".to_owned());
    let picker_record = SessionPickerRecord::from_record(&record);

    assert_eq!(picker_record.title, "previous title");
}

#[test]
fn new_agent_envelope_title_uses_sender_identity_and_first_body_line() {
    let sender = serde_json::json!({
        "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"claude-local"},
        "sessionId":"sender-session"
    });
    let recipient = serde_json::json!({
        "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"codex-local"},
        "sessionId":"recipient-session"
    });
    let envelope = format!(
        "🤖 Codex Main ← 🐒 Sidekick · PR2\nAgent communication\nSelf-declared sender: {sender}\nIntended recipient: {recipient}\n\nFix empty-session visibility.\nIgnore later body lines.",
    );
    let mut record = search_consistency_record(None, None);
    record.title = Some(envelope);

    let picker_record = SessionPickerRecord::from_record(&record);

    assert_eq!(
        picker_record.title,
        "🐒 Sidekick · PR2: Fix empty-session visibility."
    );
}

#[test]
fn old_agent_envelope_title_uses_sender_fallback_and_preserves_renamed_session_name() {
    let sender = serde_json::json!({
        "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"claude-local"},
        "sessionId":"sender-session"
    });
    let recipient = serde_json::json!({
        "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"codex-local"},
        "sessionId":"recipient-session"
    });
    let envelope = format!(
        "Agent communication\nSelf-declared sender: {sender}\nIntended recipient: {recipient}\n\nFix empty-session visibility.\nIgnore later body lines.",
    );
    let mut record = search_consistency_record(None, None);
    record.name = Some("Renamed in Codex".to_owned());
    record.title = Some(envelope);

    let picker_record = SessionPickerRecord::from_record(&record);

    assert_eq!(
        picker_record.title,
        "Renamed in Codex | ✳️ claude-local/sender-s: Fix empty-session visibility."
    );
}

#[test]
fn agent_envelope_in_first_user_message_overrides_a_generic_title_but_keeps_rename() {
    let sender = serde_json::json!({
        "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"claude-local"},
        "sessionId":"sender-session"
    });
    let recipient = serde_json::json!({
        "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"codex-local"},
        "sessionId":"recipient-session"
    });
    let envelope = format!(
        "Agent communication\nSelf-declared sender: {sender}\nIntended recipient: {recipient}\n\nCheck the delivery record.\nLater body line.",
    );
    let mut record = search_consistency_record(None, None);
    record.name = Some("Durable rename".to_owned());
    record.title = Some("Agent communication".to_owned());
    record.first_user_message = Some(envelope);

    let picker_record = SessionPickerRecord::from_record(&record);

    assert_eq!(
        picker_record.title,
        "Durable rename | ✳️ claude-local/sender-s: Check the delivery record."
    );
}

#[cfg(unix)]
#[test]
fn picker_record_normalizes_existing_cwd_before_interactive_matching() {
    let fixture_root = std::env::temp_dir().join(format!(
        "codex-router-session-picker-path-normalization-{}",
        std::process::id()
    ));
    let canonical_checkout = fixture_root.join("canonical-checkout");
    let checkout_alias = fixture_root.join("checkout-alias");
    fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
    std::os::unix::fs::symlink(&canonical_checkout, &checkout_alias)
        .expect("create checkout symlink");
    let mut record = search_consistency_record(None, None);
    record.cwd = Some(checkout_alias.display().to_string());

    let picker_record = SessionPickerRecord::from_record(&record);
    let actual_cwd = picker_record.normalized_cwd;
    let expected_cwd = fs::canonicalize(&canonical_checkout)
        .expect("canonicalize checkout")
        .display()
        .to_string();
    fs::remove_file(&checkout_alias).expect("remove checkout symlink");
    fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
    fs::remove_dir(&fixture_root).expect("remove fixture root");

    assert_eq!(actual_cwd.as_deref(), Some(expected_cwd.as_str()));
}

#[test]
fn duration_format_uses_now_without_suffix_for_subminute_values() {
    assert_eq!(format_duration_ms(0), "now");
    assert_eq!(format_duration_ms(59_000), "now");
    assert_eq!(format_duration_ms(60_000), "1m");
}

#[test]
fn conversation_preview_applies_cli_snippet_truncation() {
    let root = std::env::temp_dir().join(format!(
        "agent-collaboration-preview-truncation-{}",
        std::process::id()
    ));
    let codex_home = root.join("codex-home");
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    let rollout_path = sessions.join("rollout.jsonl");
    let long_message = "x".repeat(240);
    fs::write(
        &rollout_path,
        format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"text\":\"{long_message}\"}}]}}}}"
        ),
    )
    .expect("write rollout");
    let source = SessionConversationSource::new(rollout_path.display().to_string(), codex_home);

    let preview = SessionConversationPreview::from_rollout_source(Some(&source));

    assert_eq!(preview.snippets[0].chars().count(), 180);
    assert!(preview.snippets[0].ends_with('…'));
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn session_model_choice_pairs_model_with_effort_and_marks_unknown_parts() {
    // Arrange / Act / Assert: the owner resumes from this column, so every case is shown.
    assert_eq!(
        session_model_choice(Some("gpt-5.6-sol"), Some("low")),
        "gpt-5.6-sol/low"
    );
    assert_eq!(
        session_model_choice(Some("gpt-5.6-sol"), None),
        "gpt-5.6-sol/-"
    );
    assert_eq!(session_model_choice(None, Some("high")), "-/high");
    assert_eq!(session_model_choice(None, None), "-");
    assert_eq!(session_model_choice(Some("  "), Some("")), "-");
}

#[test]
fn session_model_choice_stays_within_the_row_width_budget() {
    // Arrange
    let long_model = "m".repeat(80);

    // Act
    let rendered = session_model_choice(Some(&long_model), Some("medium"));

    // Assert
    assert_eq!(rendered.chars().count(), SESSION_MODEL_CHOICE_MAX_CHARS);
    assert!(rendered.ends_with('…'));
}

#[test]
fn human_session_row_shows_the_model_choice_between_recency_and_branch() {
    // Arrange
    let mut record = search_consistency_record(None, None);
    record.model = Some("gpt-5.6-sol".to_owned());
    record.reasoning_effort = Some("low".to_owned());
    record.git_branch = Some("feat/resume".to_owned());

    // Act
    let row = human_session_row(&record);

    // Assert
    let second_line = row.lines().nth(1).expect("row has a metadata line");
    let model_index = second_line.find("gpt-5.6-sol/low").expect("model column");
    let branch_index = second_line.find("feat/resume").expect("branch column");
    assert!(model_index < branch_index);
    assert!(second_line.contains("  id=thread-"));
}
