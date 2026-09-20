//! Exact turn observation through bounded native history pages.
use crate::NativeAdmission;
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use collaboration_protocol::SessionRef;
use serde_json::{Value, json};
use std::collections::BTreeSet;
pub(crate) struct ObservedTurn {
    pub turn: Value,
    pub model: Option<String>,
    pub effort: Option<String>,
}
pub(crate) async fn read_turn(
    admission: &NativeAdmission,
    target: &SessionRef,
    turn_id: &str,
) -> Result<Option<Value>, NativeConnectionError> {
    let schemas = admission
        .schemas()
        .ok_or(NativeConnectionError::InvalidInput)?;
    let mut connection = NativeProtocolConnection::connect(admission.backend_path()).await?;
    read_turn_pages(
        &mut connection,
        &schemas,
        String::from(target.session_id.clone()).as_str(),
        turn_id,
    )
    .await
}
pub(crate) async fn read_turn_and_choice(
    admission: &NativeAdmission,
    target: &SessionRef,
    turn_id: &str,
) -> Result<Option<ObservedTurn>, NativeConnectionError> {
    let schemas = admission
        .schemas()
        .ok_or(NativeConnectionError::InvalidInput)?;
    let mut connection = NativeProtocolConnection::connect(admission.backend_path()).await?;
    let thread_id = String::from(target.session_id.clone());
    let result = connection
        .request_validated(
            &schemas,
            NativeOperation::ReadThread,
            json!({"threadId":thread_id,"includeTurns":false}),
        )
        .await?;
    if result.pointer("/thread/id").and_then(Value::as_str) != Some(thread_id.as_str()) {
        return Err(NativeConnectionError::Protocol);
    }
    let thread = result
        .get("thread")
        .ok_or(NativeConnectionError::Protocol)?;
    let model = thread
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let effort = thread
        .get("reasoningEffort")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let turn = read_turn_pages(&mut connection, &schemas, &thread_id, turn_id).await?;
    Ok(turn.map(|turn| ObservedTurn {
        turn,
        model,
        effort,
    }))
}

async fn read_turn_pages(
    connection: &mut NativeProtocolConnection,
    schemas: &codex_native_integration::NativePayloadSchemas,
    thread_id: &str,
    turn_id: &str,
) -> Result<Option<Value>, NativeConnectionError> {
    let mut cursor = None::<String>;
    let mut observed_cursors = BTreeSet::new();
    loop {
        let result = connection
            .request_validated(
                schemas,
                NativeOperation::ListTurns,
                json!({
                    "threadId":thread_id,
                    "cursor":cursor,
                    "limit":1,
                    "sortDirection":"desc",
                    "itemsView":"full"
                }),
            )
            .await?;
        let turns = result
            .get("data")
            .and_then(Value::as_array)
            .filter(|turns| turns.len() <= 1)
            .ok_or(NativeConnectionError::Protocol)?;
        if let Some(turn) = turns
            .first()
            .filter(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))
        {
            return Ok(Some(turn.clone()));
        }
        let Some(next_cursor) = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok(None);
        };
        if !observed_cursors.insert(next_cursor.clone()) || observed_cursors.len() > 16_384 {
            return Err(NativeConnectionError::Protocol);
        }
        cursor = Some(next_cursor);
    }
}
