#[path = "delivery_matrix/native_push_correlation.rs"]
mod native_push_correlation;

use native_push_correlation::{ExpectedNativePushCorrelation, validate_native_push_correlation};
use serde_json::{Value, json};

fn expected_correlation() -> ExpectedNativePushCorrelation<'static> {
    ExpectedNativePushCorrelation {
        push_id: "push-identity",
        accepted_turn_id: "accepted-turn",
        native_item_id: "generated-native-item",
    }
}

fn native_turn() -> Value {
    json!({"id":"accepted-turn","items":[{
        "type":"userMessage","id":"generated-native-item","clientId":"push-identity"
    }]})
}

#[test]
fn accepts_distinct_native_item_and_client_push_identities() {
    assert_eq!(
        validate_native_push_correlation(&[native_turn()], expected_correlation()),
        Ok(())
    );
}

#[test]
fn rejects_native_identity_without_client_correlation() {
    let turn = json!({"id":"accepted-turn","items":[{
        "type":"userMessage","id":"push-identity"
    }]});
    assert!(validate_native_push_correlation(&[turn], expected_correlation()).is_err());
}

#[test]
fn rejects_wrong_client_correlation() {
    let mut turn = native_turn();
    turn["items"][0]["clientId"] = json!("another-push");
    assert!(validate_native_push_correlation(&[turn], expected_correlation()).is_err());
}

#[test]
fn rejects_matching_correlation_in_a_different_turn() {
    let mut turn = native_turn();
    turn["id"] = json!("another-turn");
    assert!(validate_native_push_correlation(&[turn], expected_correlation()).is_err());
}

#[test]
fn rejects_matching_correlation_on_a_different_item() {
    let mut turn = native_turn();
    turn["items"][0]["id"] = json!("another-native-item");
    assert!(validate_native_push_correlation(&[turn], expected_correlation()).is_err());
}

#[test]
fn rejects_duplicate_native_inputs_for_the_same_push() {
    assert!(
        validate_native_push_correlation(&[native_turn(), native_turn()], expected_correlation())
            .is_err()
    );
}

#[test]
fn ignores_assistant_items_with_matching_identifiers() {
    let mut turn = native_turn();
    turn["items"][0]["type"] = json!("agentMessage");
    assert!(validate_native_push_correlation(&[turn], expected_correlation()).is_err());
}

#[test]
fn rejects_empty_expected_identities() {
    let expected = ExpectedNativePushCorrelation {
        push_id: "",
        ..expected_correlation()
    };
    assert!(validate_native_push_correlation(&[native_turn()], expected).is_err());
}
