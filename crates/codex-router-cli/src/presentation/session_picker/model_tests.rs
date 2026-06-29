use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::model::SessionsPickerModel;
use crate::presentation::session_picker::test_support::picker_request;

#[test]
fn sessions_picker_model_shows_and_switches_three_filters() {
    let mut model = SessionsPickerModel::new(picker_request(), 100);

    let initial = model.render_snapshot();
    assert!(initial.contains("Root cwd"));
    assert!(initial.contains("Provider any"));
    assert!(initial.contains("Source interactive"));
    assert!(initial.contains("Feature design session"));
    assert!(!initial.contains("Subagent planning"));

    model.handle_key(SessionsPickerKey::CycleRoot);
    model.handle_key(SessionsPickerKey::CycleProvider);
    model.handle_key(SessionsPickerKey::CycleSource);

    let updated = model.render_snapshot();
    assert!(updated.contains("Root checkout"));
    assert!(updated.contains("Provider current"));
    assert!(updated.contains("Source all"));
    assert!(updated.contains("Subagent planning"));
}

#[test]
fn sessions_picker_model_searches_navigates_and_selects_visible_rows() {
    let mut model = SessionsPickerModel::new(picker_request(), 100);

    model.handle_key(SessionsPickerKey::SearchChar('f'));
    model.handle_key(SessionsPickerKey::SearchChar('e'));
    assert!(model.render_snapshot().contains("Search fe"));
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
fn sessions_picker_empty_filter_offers_start_new_session() {
    let mut request = picker_request();
    request.records.clear();
    let model = SessionsPickerModel::new(request, 100);

    let snapshot = model.render_snapshot();

    assert!(snapshot.contains("> Start new session"));
    assert!(!snapshot.contains("No sessions match these filters"));
    assert_eq!(
        model.selected_outcome(),
        Some(SessionsPickerOutcome::StartNewSession)
    );
}
