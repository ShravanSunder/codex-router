use super::*;

#[test]
fn websocket_first_frame_forwards_large_malformed_payload_without_router_policy() {
    let router = WebSocketProtocolRouter::new();
    let mut first_frame = br#"{"type":"response.create","input":"#.to_vec();
    first_frame.extend(std::iter::repeat_n(b'x', 2 * 1024 * 1024));

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new(),
        WebSocketFrame::Text(first_frame.clone()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("router must not own payload size/json validity: {error:?}"),
    };

    let WebSocketFirstFrameDecision::OpenUpstream {
        first_frame: routed_first_frame,
        ..
    } = decision;
    assert_eq!(routed_first_frame, WebSocketFrame::Text(first_frame));
}

#[test]
fn websocket_first_frame_allows_nested_auth_words_in_prompt_content() {
    let router = WebSocketProtocolRouter::new();
    let first_frame = br#"{"type":"response.create","input":[{"role":"user","content":"please print \"authorization\": \"Bearer not-a-router-token\" literally"}]}"#.to_vec();

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new(),
        WebSocketFrame::Text(first_frame.clone()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(decision) => decision,
        Err(error) => panic!("nested prompt text must not trip auth smuggling: {error:?}"),
    };

    let WebSocketFirstFrameDecision::OpenUpstream {
        first_frame: routed_first_frame,
        ..
    } = decision;
    assert_eq!(routed_first_frame, WebSocketFrame::Text(first_frame));
}

#[test]
fn websocket_first_frame_accepts_future_request_shape_without_prompt_policy() {
    let router = WebSocketProtocolRouter::new();
    let first_frame = br#"{"model":"gpt-5.5","future_codex_shape":{"kept":true}}"#.to_vec();

    let decision = match router.route_first_frame(
        WebSocketHandshakeRequest::new()
            .with_header(Header::new("Authorization", "Bearer local-token")),
        WebSocketFrame::Text(first_frame.clone()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(decision) => decision,
        Err(error) => {
            panic!("future request shape should route without prompt policy: {error:?}")
        }
    };

    let WebSocketFirstFrameDecision::OpenUpstream {
        headers,
        first_frame: routed_first_frame,
        ..
    } = decision;
    assert_eq!(
        headers.values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert_eq!(routed_first_frame, WebSocketFrame::Text(first_frame));
}
