use codex_acp_adapter::{NativeInterruptionState, NativePromptTerminal, PromptSettlement};
use serde_json::json;

#[test]
fn cancellation_settles_once_and_unknown_native_effect_blocks_followup() {
    let mut prompt = PromptSettlement::new(json!(9223372036854775807_i64))
        .unwrap_or_else(|error| panic!("prompt: {error}"));
    prompt
        .mark_dispatched()
        .unwrap_or_else(|error| panic!("dispatch: {error}"));
    prompt
        .accepted_turn("turn-a".into())
        .unwrap_or_else(|error| panic!("accept: {error}"));
    assert_eq!(prompt.request_cancel(), Some("turn-a"));
    let response = prompt
        .settle_cancel(NativeInterruptionState::Rejected)
        .unwrap_or_else(|| panic!("response"));
    assert_eq!(response["result"]["stopReason"], "cancelled");
    assert_eq!(
        response["result"]["_meta"]["codex-router/nativeInterruption"]["state"],
        "rejected"
    );
    assert!(prompt.blocks_next_prompt());
    assert!(
        prompt
            .observe_terminal(Some("other"), NativePromptTerminal::Completed)
            .is_none()
    );
    assert!(prompt.blocks_next_prompt());
    assert!(
        prompt
            .observe_terminal(Some("turn-a"), NativePromptTerminal::Completed)
            .is_none()
    );
    assert!(!prompt.blocks_next_prompt());
    assert!(
        prompt
            .settle_cancel(NativeInterruptionState::Confirmed)
            .is_none()
    );
}

#[test]
fn native_interruption_without_client_cancel_is_an_error_not_invented_stop_reason() {
    let mut prompt =
        PromptSettlement::new(json!("prompt-1")).unwrap_or_else(|error| panic!("prompt: {error}"));
    prompt
        .mark_dispatched()
        .unwrap_or_else(|error| panic!("dispatch: {error}"));
    prompt
        .accepted_turn("turn-a".into())
        .unwrap_or_else(|error| panic!("accept: {error}"));
    let response = prompt
        .observe_terminal(Some("turn-a"), NativePromptTerminal::Interrupted)
        .unwrap_or_else(|| panic!("response"));
    assert_eq!(response["error"]["code"], -32603);
    assert!(response.get("result").is_none());
}

#[test]
fn cancellation_distinguishes_unsent_and_unknown_start() {
    for dispatched in [false, true] {
        let mut prompt =
            PromptSettlement::new(json!(1)).unwrap_or_else(|error| panic!("prompt: {error}"));
        if dispatched {
            prompt
                .mark_dispatched()
                .unwrap_or_else(|error| panic!("dispatch: {error}"));
        }
        assert!(prompt.request_cancel().is_none());
        let result = prompt
            .settle_cancel(NativeInterruptionState::NotDispatched)
            .unwrap_or_else(|| panic!("response"));
        assert_eq!(
            result["result"]["_meta"]["codex-router/nativeInterruption"]["state"],
            if dispatched {
                "unknown"
            } else {
                "notDispatched"
            }
        );
    }
}
