use super::*;

#[test]
fn signed_legacy_project_only_inbox_cursors_are_rejected() {
    let key = [7_u8; 32];
    let project_id = ProjectId::generate();
    let scope = InboxScope::Project {
        project_id: project_id.clone(),
    };
    let current = encode_cursor(
        &key,
        &InboxCursor {
            operation: "inbox".to_owned(),
            scope: scope.clone(),
            read_mode: InboxReadMode::Unread,
            reader_key: "reader".to_owned(),
            upper_sequence: 10,
            last_sequence: 5,
        },
    )
    .unwrap();
    assert_eq!(
        decode_inbox_cursor(
            &key,
            Some(&current),
            &scope,
            InboxReadMode::Unread,
            "reader",
            10
        )
        .unwrap(),
        (10, 5)
    );
    let legacy = encode_cursor(
        &key,
        &serde_json::json!({
            "operation":"inbox", "project_id":project_id, "reader_key":"reader",
            "upper_sequence":10, "last_sequence":5
        }),
    )
    .unwrap();
    assert_eq!(
        decode_inbox_cursor(
            &key,
            Some(&legacy),
            &scope,
            InboxReadMode::Unread,
            "reader",
            10
        )
        .unwrap_err()
        .kind,
        BoardFailureKind::InvalidCursor
    );
}
