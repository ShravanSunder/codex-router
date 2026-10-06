//! Actions preserve source identity and the selected metadata after the view changes.
use super::picker_actions::SessionsPickerOutcome;
use super::picker_model::SessionsPickerModel;
use super::test_support::{observed_records, picker_request};
use crate::sessions::SessionPickerIdentity;
use collaboration_client::protocol::SessionRef;
use serde_json::json;

#[test]
fn picker_action_outcomes_keep_equal_native_ids_on_different_routers_distinct() {
    let mut request = picker_request();
    let template = request.records.remove(0);
    let mut actions = Vec::new();
    for service in [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
    ] {
        let mut record = template.clone();
        let target: SessionRef = serde_json::from_value(json!({
            "endpoint": {"serviceId": service, "endpointId": "codex-local"},
            "sessionId": record.session_id,
        }))
        .expect("source identity");
        record.identity = SessionPickerIdentity::HostedCodex(target);
        let mut source_request = request.clone();
        source_request.records = vec![record.clone()];
        let mut model = SessionsPickerModel::new(source_request, 120);
        assert!(model.focus_visible_identity(&record.identity));
        actions.push((
            model.activation_outcome_for_focus(),
            model.fork_outcome_for_focus(),
        ));
    }

    assert_ne!(
        actions[0].0, actions[1].0,
        "resume must retain its source Router"
    );
    assert_ne!(
        actions[0].1, actions[1].1,
        "fork must retain its source Router"
    );
}

#[test]
fn picker_actions_freeze_the_selected_rows_model_and_effort() {
    let mut request = picker_request();
    let mut record = request.records.remove(0);
    record.model = Some("gpt-6.1-sol".to_owned());
    record.reasoning_effort = Some("high".to_owned());
    request.records = vec![record.clone()];
    let mut model = SessionsPickerModel::new(request, 120);
    assert!(model.focus_visible_identity(&record.identity));
    let Some(SessionsPickerOutcome::ResumeSession(resume)) = model.activation_outcome_for_focus()
    else {
        panic!("selected row must resume");
    };
    let Some(SessionsPickerOutcome::ForkSession(fork)) = model.fork_outcome_for_focus() else {
        panic!("selected row must fork");
    };

    model.replace_records(observed_records(vec![]));
    let expected = codex_native_integration::ResumeModelChoice::from_stored_values(
        Some("gpt-6.1-sol"),
        Some("high"),
    );
    assert_eq!(resume.identity, record.identity);
    assert_eq!(fork.identity, record.identity);
    assert_eq!(resume.model_choice, expected);
    assert_eq!(fork.model_choice, expected);
}
