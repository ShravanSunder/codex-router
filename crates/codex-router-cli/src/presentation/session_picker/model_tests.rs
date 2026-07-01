use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::model::SessionsPickerModel;
use crate::presentation::session_picker::test_support::picker_record;
use crate::presentation::session_picker::test_support::picker_request;

#[test]
fn sessions_picker_model_shows_and_switches_three_filters() {
    let mut model = SessionsPickerModel::new(picker_request(), 120);

    let initial = model.render_snapshot();
    assert!(initial.contains("Scope: [📂 cwd]"));
    assert!(initial.contains("Threads: [interactive]"));
    assert!(initial.contains("Sort: [updated]"));
    assert!(initial.contains("⌘N/Ctrl-N new thread"));
    assert!(initial.contains("Feature design session"));
    assert!(!initial.contains("Subagent planning"));

    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::CycleSource);

    let updated = model.render_snapshot();
    assert!(updated.contains("Scope: [worktree]"));
    assert!(updated.contains("Threads: [all]"));
    assert!(updated.contains("Subagent planning"));
}

#[test]
fn sessions_picker_model_searches_navigates_and_selects_visible_rows() {
    let mut model = SessionsPickerModel::new(picker_request(), 100);

    model.handle_key(SessionsPickerKey::SearchChar('f'));
    model.handle_key(SessionsPickerKey::SearchChar('e'));
    assert!(model.render_snapshot().contains("Search: [fe]"));
    assert_eq!(model.selected_session_id(), Some("thread-a"));

    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::SearchBackspace);
    model.handle_key(SessionsPickerKey::SearchBackspace);
    model.handle_key(SessionsPickerKey::MoveDown);
    assert_eq!(model.selected_session_id(), Some("thread-b"));
}

#[test]
fn sessions_picker_root_filters_match_checkout_and_repo_roots() {
    let mut request = picker_request();
    request.current_dir = "/repo/project-a/pkg/src".into();
    request.checkout_root = "/repo/project-a".into();
    request.repo_roots = vec!["/repo/project-a".into(), "/repo/project-b".into()];
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
    let checkout_snapshot = model.render_snapshot();
    assert!(checkout_snapshot.contains("Current source session"));
    assert!(checkout_snapshot.contains("Sibling test session"));
    assert!(!checkout_snapshot.contains("Sibling worktree session"));

    model.handle_key(SessionsPickerKey::CycleRoot);
    let repo_snapshot = model.render_snapshot();
    assert!(repo_snapshot.contains("Current source session"));
    assert!(repo_snapshot.contains("Sibling test session"));
    assert!(repo_snapshot.contains("Sibling worktree session"));
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
        model.selected_outcome(),
        Some(SessionsPickerOutcome::StartNewSession)
    );
}
