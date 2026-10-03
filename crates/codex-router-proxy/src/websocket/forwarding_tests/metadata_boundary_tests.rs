use super::*;

#[test]
fn websocket_quota_classifier_ignores_non_error_json_with_nested_quota_token() {
    let non_error_frame = Message::text(
        r#"{"type":"response.output_text.delta","error":{"code":"usage_limit_reached"}}"#,
    );

    assert_eq!(
        provider_error_classification_from_message(&non_error_frame),
        ProviderErrorClassification::Unknown,
        "WebSocket quota containment must only trigger on complete Responses error envelopes"
    );
}

#[test]
fn websocket_quota_classifier_ignores_oversized_prefix_only_quota_token() {
    let oversized_prefix_frame = Message::text(format!(
        r#"{{"type":"error","error":{{"code":"usage_limit_reached"}} bogus, "padding":"{}"}}"#,
        "x".repeat(70 * 1024)
    ));

    assert_eq!(
        provider_error_classification_from_message(&oversized_prefix_frame),
        ProviderErrorClassification::Unknown,
        "WebSocket quota containment must not infer quota state from oversized or malformed text"
    );
}

#[test]
fn response_create_detection_is_bounded_top_level_metadata_only() {
    assert!(is_response_create(&Message::text(
        r#"{"type":"response.create","turn":1}"#,
    )));
    assert!(!is_response_create(&Message::text(
        r#"{"input":[{"content":"{\"type\":\"response.create\"}"}]}"#,
    )));

    let mut late_type = br#"{"input":""#.to_vec();
    late_type.extend(std::iter::repeat_n(b'x', 70 * 1024));
    late_type.extend(br#"","type":"response.create"}"#);
    let late_type = String::from_utf8(late_type)
        .unwrap_or_else(|error| panic!("test text should be utf-8: {error}"));
    assert!(!is_response_create(&Message::text(late_type)));
}

#[tokio::test]
async fn completion_releases_before_blocked_affinity_recording() {
    let selected_account = AccountId::new("acct_selected")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let active_reservations = RouteBandReservationBooks::default();
    let reservation_handle = {
        let mut reservations = active_reservations
            .lock()
            .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
        reservations
            .entry("responses".to_owned())
            .or_insert_with(ReservationBook::default)
            .reserve_next_at(selected_account.clone(), 1, 1)
    };
    let active_reservation_guard = ActiveReservationGuard::new(
        active_reservations.clone(),
        "responses".to_owned(),
        reservation_handle,
    );
    let active_turn_reservation = ActiveTurnReservationState::new(Some(active_reservation_guard));
    let active_sessions = || {
        let reservations = active_reservations
            .lock()
            .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
        reservations
            .get("responses")
            .map_or(0, |book| book.active_session_count(&selected_account))
    };
    assert_eq!(active_sessions(), 1);

    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let affinity_owner_context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id: selected_account.clone(),
        credential_generation: 1,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let recorder_entered = Arc::new(Notify::new());
    let recorder_release = Arc::new(Notify::new());
    let recorder = Arc::new(BlockingAsyncAffinityOwnerRecorder::new(
        Arc::clone(&recorder_entered),
        Arc::clone(&recorder_release),
    ));
    let registry = WebSocketRevocationRegistry::new();
    let session = registry.register_cancellation(TokenGeneration::new(1));
    active_turn_reservation.release_current();
    registry.note_response_completed(session.session_id);

    let record_task = tokio::spawn(async move {
        record_forwarded_websocket_metadata(
            tokio_tungstenite::tungstenite::Utf8Bytes::from_static(
                r#"{"type":"response.completed","response":{"id":"resp_1"}}"#,
            ),
            Some(&affinity_owner_context),
            Some(recorder),
            None,
        )
        .await;
    });

    tokio::time::timeout(Duration::from_secs(1), recorder_entered.notified())
        .await
        .unwrap_or_else(|_elapsed| panic!("recorder should be entered"));
    assert_eq!(
        active_sessions(),
        0,
        "completion must release the active session before affinity persistence can block"
    );

    active_turn_reservation.reserve_if_idle(2);
    assert_eq!(
        active_sessions(),
        1,
        "a new same-socket turn must be able to reserve while affinity persistence is blocked"
    );
    recorder_release.notify_one();
    record_task
        .await
        .unwrap_or_else(|error| panic!("record task should finish: {error}"));
    assert_eq!(active_sessions(), 1);
}

#[test]
fn websocket_metadata_scan_ignores_oversized_late_affinity_and_completion_fields() {
    let affinity_secret = RouterAffinityHashSecret::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}"));
    let account_id = AccountId::new("acct_selected")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let context = WebSocketAffinityOwnerContext {
        affinity_secret,
        account_id,
        credential_generation: 7,
        credit_backed_at_selection: false,
        active_reservation_guard: None,
        session_affinity_activity_handle: None,
    };
    let oversized_padding = "a".repeat(65 * 1024);
    let message = Message::text(format!(
        r#"{{"padding":"{oversized_padding}","type":"response.completed","response":{{"id":"resp_late"}}}}"#
    ));

    assert!(
        websocket_affinity_owner_record(&message, Some(&context)).is_none(),
        "late oversized affinity metadata should be forwarded but not classified"
    );
    assert!(
        !is_response_completed(&message),
        "late oversized completion metadata should be forwarded but not classified"
    );
}
