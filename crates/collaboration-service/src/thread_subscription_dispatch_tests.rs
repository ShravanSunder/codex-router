use super::*;
use chrono::{DateTime, Utc};
use collaboration_protocol::{
    MessageText, PushId, SubscriptionWaitBatch, ThreadSubscriptionWaitResult,
};
use message_board::{ActivitySequence, MessageId, PendingRootNotice, TopicId};
use serde_json::{Value, json};
use std::error::Error;

#[test]
fn large_json_rpc_id_reserves_room_for_a_complete_notice_before_wait() -> Result<(), Box<dyn Error>>
{
    let first_root = pending_root_notice()?;
    let second_root = pending_root_notice()?;
    let encoded_one_root_array = serde_json::to_vec(std::slice::from_ref(&first_root))?.len();
    let empty_id_budget = maximum_root_notice_bytes(&Value::String(String::new()))?;
    let id_length = empty_id_budget
        .checked_sub(encoded_one_root_array)
        .and_then(|bytes| bytes.checked_sub(1))
        .ok_or("empty-id response must have room for one root")?;
    let id = Value::String("x".repeat(id_length));
    let maximum_roots_bytes = maximum_root_notice_bytes(&id)?;
    if maximum_roots_bytes != encoded_one_root_array + 1 {
        return Err(format!(
            "root allowance {maximum_roots_bytes} did not equal one-root array size {encoded_one_root_array} plus one byte"
        )
        .into());
    }

    let request = json!({
        "jsonrpc":"2.0",
        "id":id,
        "method":"board/threadWait",
        "params":{
            "actor":{"kind":"human","humanId":"reader"},
            "filter":{"kind":"all"},
            "maxWaitSeconds":0,
        },
    });
    let request_frame_bytes = serde_json::to_vec(&request)?.len();
    if request_frame_bytes > MAX_CONTROL_FRAME_BYTES {
        return Err(format!(
            "large-id request frame was {request_frame_bytes} bytes, exceeding {MAX_CONTROL_FRAME_BYTES}"
        )
        .into());
    }

    let request_id = request
        .get("id")
        .ok_or("request is missing its JSON-RPC id")?;
    let first_root_for_two_root_response = first_root.clone();
    let single_root_response = notice_response(request_id, vec![first_root])?;
    let two_root_response = notice_response(
        request_id,
        vec![first_root_for_two_root_response, second_root],
    )?;
    let single_root_frame_bytes = serde_json::to_vec(&single_root_response)?.len();
    let two_root_frame_bytes = serde_json::to_vec(&two_root_response)?.len();
    if single_root_frame_bytes > MAX_CONTROL_FRAME_BYTES {
        return Err(format!(
            "one-root response frame was {single_root_frame_bytes} bytes, exceeding {MAX_CONTROL_FRAME_BYTES}"
        )
        .into());
    }
    if two_root_frame_bytes <= MAX_CONTROL_FRAME_BYTES {
        return Err(format!(
            "two-root response frame was only {two_root_frame_bytes} bytes, within the {MAX_CONTROL_FRAME_BYTES}-byte limit"
        )
        .into());
    }
    Ok(())
}

fn pending_root_notice() -> Result<PendingRootNotice, Box<dyn Error>> {
    let maximum_activity_sequence = u64::try_from(i64::MAX)?;
    Ok(PendingRootNotice::new(
        MessageId::generate(),
        TopicId::generate(),
        ActivitySequence::try_from(maximum_activity_sequence)?,
        ActivitySequence::try_from(maximum_activity_sequence)?,
        u64::MAX,
    )?)
}

fn notice_response(id: &Value, roots: Vec<PendingRootNotice>) -> Result<Value, Box<dyn Error>> {
    let push_id = PushId::try_from("01890f2e-7b4c-7cc0-98c4-000000000002".to_owned())?;
    let line = MessageText::try_from("\\".repeat(MAX_PUSH_LINE_BYTES))?;
    let held_since = DateTime::<Utc>::from_timestamp(253_402_300_799, 999_999_999)
        .ok_or("valid maximum RFC3339 timestamp")?;
    let result = ThreadSubscriptionWaitResult {
        batch: Some(SubscriptionWaitBatch::Notice {
            push_id,
            line,
            held: true,
            held_since: Some(held_since),
            draining: true,
            roots,
        }),
    };
    Ok(json!({"jsonrpc":"2.0","id":id,"result":result}))
}
