//! The response budget counts the envelope with the encoded result, inclusive of the limit.
use super::ResultByteBudget;
use serde_json::json;

#[test]
fn a_result_fits_when_envelope_and_encoding_reach_the_limit_exactly() {
    // Arrange: `{"a":"bc"}` encodes to 10 bytes.
    let result = json!({"a":"bc"});
    let encoded = serde_json::to_vec(&result).expect("result encodes").len();

    // Act & assert.
    assert_eq!(
        ResultByteBudget::new(100, 7).response_bytes(&result),
        Some(encoded + 7)
    );
    assert!(ResultByteBudget::new(encoded + 7, 7).admits(&result));
    assert!(!ResultByteBudget::new(encoded + 6, 7).admits(&result));
}

#[test]
fn an_envelope_larger_than_the_limit_admits_nothing() {
    // Arrange.
    let budget = ResultByteBudget::new(4, 10);

    // Act & assert.
    assert!(!budget.admits(&json!(null)));
    assert_eq!(budget.response_limit_bytes(), 4);
}
