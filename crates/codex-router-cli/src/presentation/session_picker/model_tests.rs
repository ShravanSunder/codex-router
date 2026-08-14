use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::model::SessionsPickerModel;
use crate::presentation::session_picker::test_support::picker_record;
use crate::presentation::session_picker::test_support::picker_request;

#[test]
fn sessions_picker_model_shows_and_switches_three_filters() {
    let mut model = SessionsPickerModel::new(picker_request(), 120);

    let initial = model.render_snapshot();
    assert!(initial.contains("[📂 cwd]"));
    assert!(initial.contains("Threads: [interactive]"));
    assert!(initial.contains("Sort: [updated]"));
    assert!(initial.contains("ctrl-n new thread"));
    assert!(initial.contains("Start new session"));
    assert!(initial.contains("Feature design session"));
    assert!(!initial.contains("Subagent planning"));

    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::CycleSource);
    model.handle_key(SessionsPickerKey::CycleSort);

    let updated = model.render_snapshot();
    assert!(updated.contains("[repo]"));
    assert!(updated.contains("Threads: [all]"));
    assert!(updated.contains("Sort: [created]"));
    assert!(updated.contains("Subagent planning"));
}

#[test]
fn sessions_picker_model_cycles_scope_source_and_sort_without_focus_mode() {
    let mut model = SessionsPickerModel::new(picker_request(), 120);

    for expected_scope in ["repo", "all", "📂 cwd"] {
        model.handle_key(SessionsPickerKey::CycleRoot);
        assert!(
            model
                .render_snapshot()
                .contains(&format!("[{expected_scope}]"))
        );
    }

    for expected_threads in ["all", "subagents", "interactive"] {
        model.handle_key(SessionsPickerKey::CycleSource);
        assert!(
            model
                .render_snapshot()
                .contains(&format!("Threads: [{expected_threads}]"))
        );
    }

    model.handle_key(SessionsPickerKey::CycleSort);
    assert!(model.render_snapshot().contains("Sort: [created]"));
    model.handle_key(SessionsPickerKey::CycleSort);
    assert!(model.render_snapshot().contains("Sort: [updated]"));
}

#[test]
fn sessions_picker_model_searches_navigates_and_selects_visible_rows() {
    let mut model = SessionsPickerModel::new(picker_request(), 100);

    model.handle_key(SessionsPickerKey::SearchChar('f'));
    model.handle_key(SessionsPickerKey::SearchChar('e'));
    assert!(model.render_snapshot().contains("Search: [fe]"));
    assert!(model.render_snapshot().contains("Feature design session"));

    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::SearchBackspace);
    model.handle_key(SessionsPickerKey::SearchBackspace);
    model.handle_key(SessionsPickerKey::MoveDown);
    assert_eq!(model.focused_session_id(), Some("thread-b"));
}

#[test]
fn sessions_picker_model_uses_qualified_search_over_complete_fields() {
    let mut request = picker_request();
    request.root = crate::presentation::session_picker::request::SessionsPickerRoot::Any;
    request.records[0].branch = "feature/session-picker".to_owned();
    request.records[0].persisted_branch = "feature/session-picker".to_owned();
    request.records[0].full_title = "Conversation beyond compact display".to_owned();
    request.records[0].first_user_message = "find the historical worktree".to_owned();
    request.records[0].git_origin_url =
        Some("https://github.com/shravan-agent/codex-router.git".to_owned());
    let mut model = SessionsPickerModel::new(request, 100);

    for character in "b:session-picker historical".chars() {
        model.handle_key(SessionsPickerKey::SearchChar(character));
    }
    assert!(model.render_snapshot().contains("Feature design session"));

    model.handle_key(SessionsPickerKey::ClearSearch);
    for character in "session-picker".chars() {
        model.handle_key(SessionsPickerKey::SearchChar(character));
    }
    assert!(!model.render_snapshot().contains("Feature design session"));
}

#[test]
fn sessions_picker_model_scrolls_visible_window_with_selection() {
    let mut request = picker_request();
    request.root = crate::presentation::session_picker::request::SessionsPickerRoot::Any;
    request.source = crate::sessions::SessionsSource::All;
    for index in 0..12 {
        request.records.push(picker_record(
            &format!("thread-extra-{index}"),
            &format!("Overflow session {index}"),
            "/repo/project-a",
            "codex-router",
            "cli",
        ));
    }
    let mut model = SessionsPickerModel::new(request, 100);

    for _ in 0..10 {
        model.handle_key(SessionsPickerKey::MoveDown);
    }
    let snapshot = model.render_snapshot();

    assert!(
        snapshot.contains("more above"),
        "scrolling list should show records above selected row:\n{snapshot}"
    );
    assert!(
        snapshot.contains("❯ Overflow session 7"),
        "selected row should stay visible after moving past first page:\n{snapshot}"
    );
}

#[test]
fn sessions_picker_model_supports_page_and_edge_navigation() {
    let mut request = picker_request();
    request.root = crate::presentation::session_picker::request::SessionsPickerRoot::Any;
    request.source = crate::sessions::SessionsSource::All;
    for index in 0..12 {
        request.records.push(picker_record(
            &format!("thread-extra-{index}"),
            &format!("Overflow session {index}"),
            "/repo/project-a",
            "codex-router",
            "cli",
        ));
    }
    let mut model = SessionsPickerModel::new(request, 100);

    model.handle_key(SessionsPickerKey::PageDown);
    assert_eq!(model.focused_session_id(), Some("thread-extra-5"));
    model.handle_key(SessionsPickerKey::MoveLast);
    assert_eq!(model.focused_session_id(), Some("thread-extra-11"));
    model.handle_key(SessionsPickerKey::PageUp);
    assert_eq!(model.focused_session_id(), Some("thread-extra-3"));
    model.handle_key(SessionsPickerKey::MoveFirst);
    assert_eq!(
        model.activation_outcome_for_focus(),
        Some(SessionsPickerOutcome::StartNewSession)
    );
}

#[test]
fn sessions_picker_model_reuses_visible_rows_for_navigation_keys() {
    let mut request = picker_request();
    request.root = crate::presentation::session_picker::request::SessionsPickerRoot::Any;
    request.source = crate::sessions::SessionsSource::All;
    for index in 0..25 {
        request.records.push(picker_record(
            &format!("thread-extra-{index}"),
            &format!("Overflow session {index}"),
            "/repo/project-a",
            "codex-router",
            "cli",
        ));
    }
    let mut model = SessionsPickerModel::new(request, 100);

    let initial_generation = model.visible_rows_generation();
    model.handle_key(SessionsPickerKey::MoveDown);
    model.handle_key(SessionsPickerKey::MoveDown);
    model.handle_key(SessionsPickerKey::PageDown);

    assert_eq!(
        model.visible_rows_generation(),
        initial_generation,
        "navigation should not rebuild the filtered/sorted visible rows"
    );

    model.handle_key(SessionsPickerKey::CycleRoot);
    assert!(
        model.visible_rows_generation() > initial_generation,
        "filter changes should rebuild the visible rows"
    );
}

#[test]
fn sessions_picker_model_clears_search_without_changing_filters() {
    let mut model = SessionsPickerModel::new(picker_request(), 100);

    model.handle_key(SessionsPickerKey::SearchChar('r'));
    model.handle_key(SessionsPickerKey::SearchChar('u'));
    model.handle_key(SessionsPickerKey::SearchChar('s'));
    model.handle_key(SessionsPickerKey::SearchChar('t'));
    assert!(model.render_snapshot().contains("Search: [rust]"));

    model.handle_key(SessionsPickerKey::ClearSearch);
    let snapshot = model.render_snapshot();
    assert!(snapshot.contains("Search text, id:, b:branch, repo:name"));
    assert!(snapshot.contains("[📂 cwd]"));
    assert!(snapshot.contains("Threads: [interactive]"));
}

#[test]
fn sessions_picker_root_filters_match_cwd_and_repository_identity() {
    let mut request = picker_request();
    request.current_dir = "/repo/project-a/pkg/src".into();
    request.repository_identity.live_roots =
        vec!["/repo/project-a".into(), "/repo/project-b".into()];
    request.records = vec![
        picker_record(
            "thread-src",
            "Current source session",
            "/repo/project-a/pkg/src",
            "codex-router",
            "cli",
        ),
        picker_record(
            "thread-tests",
            "Sibling test session",
            "/repo/project-a/pkg/tests",
            "codex-router",
            "cli",
        ),
        picker_record(
            "thread-worktree",
            "Sibling worktree session",
            "/repo/project-b",
            "codex-router",
            "cli",
        ),
    ];

    let mut model = SessionsPickerModel::new(request, 100);
    let cwd_snapshot = model.render_snapshot();
    assert!(cwd_snapshot.contains("Current source session"));
    assert!(!cwd_snapshot.contains("Sibling test session"));
    assert!(!cwd_snapshot.contains("Sibling worktree session"));

    model.handle_key(SessionsPickerKey::CycleRoot);
    let repo_snapshot = model.render_snapshot();
    assert!(repo_snapshot.contains("Current source session"));
    assert!(repo_snapshot.contains("Sibling test session"));
    assert!(repo_snapshot.contains("Sibling worktree session"));
}

#[test]
fn sessions_picker_model_keeps_start_new_choice_with_existing_sessions() {
    let mut request = picker_request();
    request.new_session_args_display = "--yolo --model gpt-5-codex".to_owned();
    let mut model = SessionsPickerModel::new(request, 120);

    let initial = model.render_snapshot();
    assert!(initial.contains("Start new session"));
    assert!(initial.contains("args: --yolo --model gpt-5-codex"));
    assert_eq!(model.focused_session_id(), Some("thread-a"));

    model.handle_key(SessionsPickerKey::MoveUp);
    assert_eq!(
        model.activation_outcome_for_focus(),
        Some(SessionsPickerOutcome::StartNewSession)
    );
}

#[test]
fn sessions_picker_empty_filter_offers_start_new_session() {
    let mut request = picker_request();
    request.records.clear();
    let model = SessionsPickerModel::new(request, 100);

    let snapshot = model.render_snapshot();

    assert!(snapshot.contains("Start new session"));
    assert!(!snapshot.contains("No sessions match these filters"));
    assert_eq!(
        model.activation_outcome_for_focus(),
        Some(SessionsPickerOutcome::StartNewSession)
    );
}

#[test]
fn sessions_picker_pointer_focus_resolves_stable_visible_session_identity() {
    let mut request = picker_request();
    request.root = crate::presentation::session_picker::request::SessionsPickerRoot::Any;
    request.source = crate::sessions::SessionsSource::All;
    let mut model = SessionsPickerModel::new(request, 100);

    assert!(model.focus_visible_session("thread-b"));
    assert_eq!(model.focused_session_id(), Some("thread-b"));
    assert_eq!(
        model.activation_outcome_for_focus(),
        Some(SessionsPickerOutcome::ResumeSession("thread-b".to_owned()))
    );

    let mut replacement_records = model.request.records.clone();
    replacement_records.reverse();
    model.replace_records(replacement_records);

    assert_eq!(
        model.focused_session_id(),
        Some("thread-b"),
        "record reloads must preserve pointer focus by session identity"
    );
}

#[test]
fn sessions_picker_pointer_focus_ignores_stale_identity_and_keeps_start_new_explicit() {
    let mut model = SessionsPickerModel::new(picker_request(), 100);
    let initial_session_id = model.focused_session_id().map(str::to_owned);

    assert!(!model.focus_visible_session("missing-session"));
    assert_eq!(
        model.focused_session_id().map(str::to_owned),
        initial_session_id
    );

    model.focus_start_new();
    assert_eq!(model.focused_session_id(), None);
    assert_eq!(
        model.activation_outcome_for_focus(),
        Some(SessionsPickerOutcome::StartNewSession)
    );
}

#[test]
fn sessions_picker_pointer_focus_preserves_the_rendered_scrolled_window() {
    let mut request = picker_request();
    request.root = crate::presentation::session_picker::request::SessionsPickerRoot::Any;
    request.source = crate::sessions::SessionsSource::All;
    for index in 0..12 {
        request.records.push(picker_record(
            &format!("thread-window-{index}"),
            &format!("Window row {index}"),
            "/repo/project-a",
            "codex-router",
            "cli",
        ));
    }
    let mut model = SessionsPickerModel::new(request, 100);
    model.handle_key(SessionsPickerKey::MoveLast);
    let window_start = model.focused_window_start(super::model::VISIBLE_SESSION_ROWS);
    let session_id = model
        .visible_choice_record_at(window_start)
        .map(|record| record.session_id.clone())
        .unwrap_or_else(|| panic!("scrolled window should start on an existing session"));

    assert!(model.focus_visible_session_in_window(&session_id, Some(window_start)));
    assert_eq!(
        model.focused_window_start(super::model::VISIBLE_SESSION_ROWS),
        window_start,
        "pointer focus must not move a row that was already visible"
    );
}
