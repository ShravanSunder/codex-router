//! Validate the native client's persisted push correlation independently of I/O.
use serde_json::Value;

pub(crate) struct ExpectedNativePushCorrelation<'a> {
    pub push_id: &'a str,
    pub accepted_turn_id: &'a str,
    pub native_item_id: &'a str,
}

pub(crate) fn validate_native_push_correlation(
    turns: &[Value],
    expected: ExpectedNativePushCorrelation<'_>,
) -> Result<(), &'static str> {
    if expected.push_id.is_empty()
        || expected.accepted_turn_id.is_empty()
        || expected.native_item_id.is_empty()
    {
        return Err("push correlation requires nonempty identities");
    }
    let mut matching_items = 0;
    for turn in turns {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            if item.get("type").and_then(Value::as_str) != Some("userMessage")
                || item.get("clientId").and_then(Value::as_str) != Some(expected.push_id)
            {
                continue;
            }
            matching_items += 1;
            if turn.get("id").and_then(Value::as_str) != Some(expected.accepted_turn_id) {
                return Err("persisted push correlation belongs to a different turn");
            }
            if item.get("id").and_then(Value::as_str) != Some(expected.native_item_id) {
                return Err("persisted push correlation belongs to a different native item");
            }
        }
    }
    if matching_items != 1 {
        return Err("persisted push correlation must identify exactly one user message");
    }
    Ok(())
}
