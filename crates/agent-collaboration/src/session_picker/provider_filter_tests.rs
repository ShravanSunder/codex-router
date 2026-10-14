//! Literal identity oracles for typed family partitions and transient draft behavior.
use super::{
    picker_model::SessionsPickerModel,
    provider_filter_model::{ProviderFilterFocus, SessionProviderFamily, SessionProviderFilter},
    test_support::{observed_records, picker_record, picker_request},
};
use crate::sessions::{SessionPickerIdentity, SessionPickerRecord};
use collaboration_client::protocol::{EndpointDescription, ProviderKind, ProviderSessionSummary};

fn provider_summary(origin: &str, id: &str, endpoint: &str) -> ProviderSessionSummary {
    let target = serde_json::json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":endpoint},"sessionId":id});
    let value = if origin == "claudeCodeInteractive" {
        serde_json::json!({"origin":origin,"target":target,"workingDirectory":"/repo/project-a","updatedAt":3,"startedAt":1,"name":id,"status":"idle","statusUpdatedAt":3,"kind":"interactive","entrypoint":"cli"})
    } else {
        serde_json::json!({"origin":origin,"target":target,"workingDirectory":"/repo/project-a","updatedAt":3,"state":"requiresAction","approver":{"kind":"human","humanId":"owner"},"createdBy":target})
    };
    serde_json::from_value(value).unwrap()
}

fn description(summary: &ProviderSessionSummary, kinds: &[ProviderKind]) -> EndpointDescription {
    let channels = kinds.iter().map(|kind| serde_json::json!({"kind":"externalProvider","transport":"stdioAcp","bindingId":"fixture-binding","bindingGeneration":7,"runtime":{"provider":kind,"runtimeName":"fixture"},"capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]})).collect::<Vec<_>>();
    let availability = if kinds.is_empty() {
        serde_json::json!({"state":"unavailable","observedAt":"2026-09-07T00:00:00Z","reason":"fixture has no runtime evidence"})
    } else {
        serde_json::json!({"state":"available","observedAt":"2026-09-07T00:00:00Z"})
    };
    serde_json::from_value(serde_json::json!({"endpoint":summary.target().endpoint,"label":"Misleading Codex Cursor Claude label","availability":availability,"channels":channels})).unwrap()
}

fn provider_row(id: &str, kind: ProviderKind) -> SessionPickerRecord {
    let summary = provider_summary("hostedProvider", id, id);
    SessionPickerRecord::from_provider_summary(&summary, id)
        .with_provider_endpoint(&summary, &description(&summary, &[kind]))
}

pub(super) fn mixed_request() -> super::SessionsPickerRequest {
    let mut request = picker_request();
    request.root = super::SessionsPickerRoot::Any;
    request.source = crate::sessions::SessionsSource::All;
    let unknown = provider_summary("hostedProvider", "unknown", "unknown");
    request.records = vec![
        picker_record(
            "native-a",
            "Native Alpha",
            "/repo/project-a",
            "claude",
            "cli",
        ),
        picker_record(
            "native-b",
            "Native Beta",
            "/repo/project-a",
            "cursor",
            "cli",
        ),
        provider_row("claude", ProviderKind::ClaudeCode),
        provider_row("cursor", ProviderKind::Cursor),
        SessionPickerRecord::from_provider_summary(&unknown, "Claude Code guessed label"),
    ];
    request
}

fn visible_ids(model: &SessionsPickerModel) -> Vec<&str> {
    (0..model.visible_record_len())
        .map(|index| model.visible_record_at(index).unwrap().session_id.as_str())
        .collect()
}

fn apply(model: &mut SessionsPickerModel, selection: SessionProviderFilter) {
    model.open_provider_filters();
    model.provider_filter_draft.as_mut().unwrap().selection = selection;
    model.apply_provider_filters();
}

#[test]
fn provider_filters_use_literal_family_partitions_and_preserve_queries() {
    let mut model = SessionsPickerModel::new(mixed_request(), 120);
    let query = model.data_query();
    for (selection, mut expected) in [
        (
            SessionProviderFilter::All,
            vec!["native-a", "native-b", "claude", "cursor", "unknown"],
        ),
        (SessionProviderFilter::Codex, vec!["native-a", "native-b"]),
        (SessionProviderFilter::ClaudeCode, vec!["claude"]),
        (SessionProviderFilter::Cursor, vec!["cursor"]),
    ] {
        apply(&mut model, selection);
        let mut actual = visible_ids(&model);
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
        assert_eq!(model.data_query(), query);
        assert_eq!(model.request.records.len(), 5);
    }
}

#[test]
fn provider_filters_retain_full_identity_fall_back_and_survive_reload_during_draft() {
    let mut request = mixed_request();
    let mut other = provider_row("claude", ProviderKind::ClaudeCode);
    let SessionPickerIdentity::HostedProvider(target) = &mut other.identity else {
        panic!("provider fixture");
    };
    target.endpoint.endpoint_id = "other-source".to_owned().try_into().unwrap();
    let expected_identity = other.identity.clone();
    request.records.push(other);
    let mut model = SessionsPickerModel::new(request, 120);
    assert!(model.focus_visible_identity(&expected_identity));
    apply(&mut model, SessionProviderFilter::ClaudeCode);
    assert_eq!(model.focused_identity(), Some(&expected_identity));
    let generation = model.visible_rows_generation();
    model.open_provider_filters();
    model.provider_filter_draft.as_mut().unwrap().selection = SessionProviderFilter::Cursor;
    model.replace_records(observed_records(model.request.records.clone()));
    assert_eq!(model.provider_filter, SessionProviderFilter::ClaudeCode);
    assert_eq!(
        model.provider_filter_draft.unwrap().selection,
        SessionProviderFilter::Cursor
    );
    model.apply_provider_filters();
    assert_eq!(model.focused_session_id(), Some("cursor"));
    assert!(model.visible_rows_generation() > generation);
    model.replace_records(observed_records(vec![]));
    assert_eq!(model.provider_filter, SessionProviderFilter::Cursor);
    assert_eq!(model.focused_identity(), None);
}

#[test]
fn provider_filters_cancel_and_modal_open_guards_preserve_focus() {
    let mut model = SessionsPickerModel::new(mixed_request(), 120);
    let identity = model.focused_identity().cloned();
    model.open_provider_filters();
    let draft = model.provider_filter_draft.as_mut().unwrap();
    draft.move_provider(1);
    draft.move_focus(false);
    assert_eq!(draft.focus, ProviderFilterFocus::Apply);
    draft.move_focus(false);
    assert_eq!(draft.focus, ProviderFilterFocus::Cancel);
    draft.move_focus(true);
    assert_eq!(draft.focus, ProviderFilterFocus::Apply);
    model.provider_filter_draft = None;
    assert_eq!(model.provider_filter, SessionProviderFilter::All);
    assert_eq!(model.focused_identity(), identity.as_ref());
    model.open_fork_confirmation();
    model.open_provider_filters();
    assert!(model.provider_filter_draft.is_none());
    model.fork_confirmation = None;
    model
        .machine_controls
        .open_browse(&model.request.router_registry);
    model.open_provider_filters();
    assert!(model.provider_filter_draft.is_none());
}

#[test]
fn provider_filter_projection_rejects_ambiguous_conflicting_missing_and_wrong_endpoint_evidence() {
    for (origin, kinds, expected) in [
        (
            "hostedProvider",
            vec![ProviderKind::Cursor],
            SessionProviderFamily::External(ProviderKind::Cursor),
        ),
        (
            "hostedProvider",
            vec![],
            SessionProviderFamily::Unclassified,
        ),
        (
            "hostedProvider",
            vec![ProviderKind::Cursor, ProviderKind::Cursor],
            SessionProviderFamily::Unclassified,
        ),
        (
            "hostedProvider",
            vec![ProviderKind::ClaudeCode, ProviderKind::Cursor],
            SessionProviderFamily::Unclassified,
        ),
        (
            "claudeCodeInteractive",
            vec![ProviderKind::Cursor],
            SessionProviderFamily::Unclassified,
        ),
        (
            "claudeCodeInteractive",
            vec![ProviderKind::ClaudeCode],
            SessionProviderFamily::External(ProviderKind::ClaudeCode),
        ),
    ] {
        let summary = provider_summary(origin, "same-id", "actual");
        let endpoint = description(&summary, &kinds);
        let record = SessionPickerRecord::from_provider_summary(&summary, "Misleading label")
            .with_provider_endpoint(&summary, &endpoint);
        assert_eq!(record.client_provider_family(), expected);
    }
    let summary = provider_summary("claudeCodeInteractive", "interactive", "actual");
    assert_eq!(
        SessionPickerRecord::from_provider_summary(&summary, "Cursor").client_provider_family(),
        SessionProviderFamily::External(ProviderKind::ClaudeCode)
    );
    let mut endpoint = description(&summary, &[ProviderKind::ClaudeCode]);
    endpoint.endpoint.endpoint_id = "different".to_owned().try_into().unwrap();
    assert_eq!(
        SessionPickerRecord::from_provider_summary(&summary, "Claude")
            .with_provider_endpoint(&summary, &endpoint)
            .client_provider_family(),
        SessionProviderFamily::Unclassified
    );
    let mut native = picker_record("native", "Native", "/repo", "claude", "cli");
    native.provider_family = SessionProviderFamily::External(ProviderKind::Cursor);
    assert_eq!(
        native.client_provider_family(),
        SessionProviderFamily::Codex
    );
}
