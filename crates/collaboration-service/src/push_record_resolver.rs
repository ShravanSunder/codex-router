//! Resolves local push links, applies reported-caller access rules, and expands stored ranges.
use crate::ServiceIdentity;
use automation_storage::{DirectMessageHistoryQuery, PushInboxQuery, StorageError};
use collaboration_protocol::{
    MachineId, PushActivityRangeRead, PushId, PushKind, PushMessageSendResult, PushOrigin,
    PushRecord, PushRecordHistoryParams, PushRecordListParams, PushRecordListResult,
    PushRecordNotice, PushRecordShowParams, PushRecordShowResult, RouterLink, SessionRef,
    PushLineInput, render_push_line,
};
use message_board::{MessageListRequest, MessageListScope, MessageSelection, PageLimit, PageRequest};
use serde_json::{Value, json};

pub(crate) fn link_for(record: &PushRecord, identity: &ServiceIdentity) -> String {
    RouterLink::new(
        MachineId::from(identity.machine_identity.service_id().clone()),
        record.push_id.clone(),
    )
    .to_string()
}

pub(crate) fn line_for(
    record: &PushRecord,
    identity: &ServiceIdentity,
) -> Result<String, collaboration_protocol::PushLineError> {
    render_push_line(&PushLineInput {
        link: RouterLink::new(
            MachineId::from(identity.machine_identity.service_id().clone()),
            record.push_id.clone(),
        ),
        machine_label: identity.machine_identity.machine_label().clone(),
        origin: record.origin.clone(),
        header_facts: record.header_facts.clone(),
        body: record.body.clone(),
    })
}

pub(crate) fn delivery_result(
    record: &PushRecord,
    target_identity: String,
    identity: &ServiceIdentity,
) -> Result<PushMessageSendResult, collaboration_protocol::PushLineError> {
    Ok(PushMessageSendResult {
        push_id: record.push_id.clone(),
        link: link_for(record, identity),
        target: record.target.clone(),
        target_identity,
        receipt: record
            .last_outcome
            .clone()
            .ok_or(collaboration_protocol::PushLineError::InvalidHeaderFacts)?,
    })
}

pub(crate) async fn show(
    id: Value,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let params = match serde_json::from_value::<PushRecordShowParams>(params) {
        Ok(params) => params,
        Err(_) => return failure(id, -32602, "invalidField", "inspect", "Invalid push show request"),
    };
    if !caller_is_local(&params.caller, identity) {
        return failure(id, -32602, "wrongService", "inspect", "Caller belongs to another Router");
    }
    let push_id = match resolve_reference(&params.reference, identity) {
        Ok(push_id) => push_id,
        Err(ReferenceError::Invalid) => {
            return failure(id, -32602, "invalidField", "inspect", "Invalid push id or Router link");
        }
        Err(ReferenceError::Foreign(machine_id)) => {
            return failure(
                id,
                -32050,
                "foreignMachine",
                "inspect",
                &format!("lives on {machine_id}; cross-machine fetch not available yet"),
            );
        }
    };
    let Some(store) = identity.automation.as_ref() else {
        return failure(id, -32050, "unavailable", "inspect", "Push storage is unavailable");
    };
    let mut record = match store.lock().await.get_push_record(&push_id).await {
        Ok(Some(record)) => record,
        Ok(None) | Err(StorageError::PushNotFound) => {
            return not_found(id, "inspect");
        }
        Err(_) => return failure(id, -32050, "unavailable", "inspect", "Push storage could not be read"),
    };
    if !can_read(&record, &params.caller) {
        return failure(id, -32050, "notPermitted", "inspect", "Not permitted to read this push");
    }
    if record.kind == PushKind::DirectMessage && record.target == params.caller {
        record = match store
            .lock()
            .await
            .mark_push_read(&push_id, &params.caller, chrono::Utc::now())
            .await
        {
            Ok(record) => record,
            Err(StorageError::PushNotFound) => return not_found(id, "inspect"),
            Err(StorageError::PushNotPermitted) => {
                return failure(id, -32050, "notPermitted", "inspect", "Not permitted to read this push");
            }
            Err(_) => return failure(id, -32050, "unavailable", "inspect", "Push read state could not be recorded"),
        };
    }
    let activity_ranges = match expand_activity_ranges(&record, identity).await {
        Ok(ranges) => ranges,
        Err(ActivityReadError::Unavailable) => {
            return failure(id, -32050, "unavailable", "inspect", "Board range storage is unavailable");
        }
        Err(ActivityReadError::Failed) => {
            return failure(id, -32050, "unavailable", "inspect", "Board activity range could not be read");
        }
    };
    let result = PushRecordShowResult {
        link: link_for(&record, identity),
        record,
        activity_ranges,
    };
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

pub(crate) async fn inbox(
    id: Value,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let params = match serde_json::from_value::<PushRecordListParams>(params) {
        Ok(params) => params,
        Err(_) => return failure(id, -32602, "invalidField", "discovery", "Invalid message inbox request"),
    };
    if !caller_is_local(&params.caller, identity) {
        return failure(id, -32602, "wrongService", "discovery", "Caller belongs to another Router");
    }
    let Some(store) = identity.automation.as_ref() else {
        return failure(id, -32050, "unavailable", "discovery", "Push storage is unavailable");
    };
    let records = match store
        .lock()
        .await
        .list_direct_message_inbox(&PushInboxQuery {
            target: params.caller,
            limit: params.limit,
        })
        .await
    {
        Ok(records) => records,
        Err(StorageError::InvalidRecord) => {
            return failure(id, -32602, "invalidField", "discovery", "Inbox limit must be between 1 and 100");
        }
        Err(_) => return failure(id, -32050, "unavailable", "discovery", "Push inbox could not be read"),
    };
    match make_notice_list(records, identity) {
        Ok(records) => json!({"jsonrpc":"2.0","id":id,"result":PushRecordListResult { records }}),
        Err(_) => failure(id, -32050, "unavailable", "discovery", "Push notice could not be rendered"),
    }
}

pub(crate) async fn history(
    id: Value,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let params = match serde_json::from_value::<PushRecordHistoryParams>(params) {
        Ok(params) => params,
        Err(_) => return failure(id, -32602, "invalidField", "discovery", "Invalid message history request"),
    };
    if !caller_is_local(&params.caller, identity) || !caller_is_local(&params.with, identity) {
        return failure(id, -32602, "wrongService", "discovery", "Both sessions must belong to this Router");
    }
    let Some(store) = identity.automation.as_ref() else {
        return failure(id, -32050, "unavailable", "discovery", "Push storage is unavailable");
    };
    let records = match store
        .lock()
        .await
        .list_direct_message_history(&DirectMessageHistoryQuery {
            caller: params.caller,
            with: params.with,
            limit: params.limit,
        })
        .await
    {
        Ok(records) => records,
        Err(StorageError::InvalidRecord) => {
            return failure(id, -32602, "invalidField", "discovery", "History limit must be between 1 and 100");
        }
        Err(_) => return failure(id, -32050, "unavailable", "discovery", "Push history could not be read"),
    };
    match make_notice_list(records, identity) {
        Ok(records) => json!({"jsonrpc":"2.0","id":id,"result":PushRecordListResult { records }}),
        Err(_) => failure(id, -32050, "unavailable", "discovery", "Push notice could not be rendered"),
    }
}

pub(crate) fn resolve_reference(
    reference: &str,
    identity: &ServiceIdentity,
) -> Result<PushId, ReferenceError> {
    if let Ok(push_id) = PushId::try_from(reference.to_owned()) {
        return Ok(push_id);
    }
    let link = RouterLink::parse(reference).map_err(|_| ReferenceError::Invalid)?;
    let local_machine_id = String::from(identity.machine_identity.service_id().clone());
    if link.machine_id().as_str() != local_machine_id {
        return Err(ReferenceError::Foreign(link.machine_id().as_str().to_owned()));
    }
    Ok(link.push_id().clone())
}

pub(crate) fn can_read(record: &PushRecord, caller: &SessionRef) -> bool {
    if &record.target == caller {
        return true;
    }
    record.kind == PushKind::DirectMessage
        && matches!(&record.origin, PushOrigin::Session(origin) if origin == caller)
}

fn caller_is_local(caller: &SessionRef, identity: &ServiceIdentity) -> bool {
    caller.endpoint.service_id == identity.service_id
}

fn make_notice_list(
    records: Vec<PushRecord>,
    identity: &ServiceIdentity,
) -> Result<Vec<PushRecordNotice>, collaboration_protocol::PushLineError> {
    records
        .into_iter()
        .map(|record| {
            Ok(PushRecordNotice {
                push_id: record.push_id.clone(),
                link: link_for(&record, identity),
                line: line_for(&record, identity)?,
                origin: record.origin.clone(),
                target: record.target.clone(),
                reply_to_push_id: record.reply_to_push_id.clone(),
                delivery_state: record.delivery_state,
                created_at: record.created_at,
                read_at: record.read_at,
            })
        })
        .collect()
}

enum ActivityReadError {
    Unavailable,
    Failed,
}

async fn expand_activity_ranges(
    record: &PushRecord,
    identity: &ServiceIdentity,
) -> Result<Vec<PushActivityRangeRead>, ActivityReadError> {
    let Some(snapshot) = record.activity.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(board) = identity.board.as_ref() else {
        return Err(ActivityReadError::Unavailable);
    };
    let limit = PageLimit::try_from(100).map_err(|_| ActivityReadError::Failed)?;
    let mut ranges = Vec::with_capacity(snapshot.ranges.len());
    for range in &snapshot.ranges {
        let mut cursor = None;
        let mut messages = Vec::new();
        loop {
            let page = board
                .lock()
                .await
                .list_messages(MessageListRequest {
                    scope: MessageListScope::Thread {
                        root_message_id: range.root_message_id.clone(),
                    },
                    selection: MessageSelection::Range {
                        from_activity_sequence: range.from_activity_sequence,
                        to_activity_sequence: range.through_activity_sequence,
                    },
                    page: PageRequest {
                        limit,
                        cursor: cursor.clone(),
                    },
                })
                .await
                .map_err(|_| ActivityReadError::Failed)?;
            messages.extend(page.page.records);
            let Some(next_cursor) = page.page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        ranges.push(PushActivityRangeRead {
            range: range.clone(),
            messages,
        });
    }
    Ok(ranges)
}

pub(crate) enum ReferenceError {
    Invalid,
    Foreign(String),
}

pub(crate) fn failure(
    id: Value,
    code: i64,
    kind: &str,
    stage: &str,
    message: &str,
) -> Value {
    json!({
        "jsonrpc":"2.0",
        "id":id,
        "error":{"code":code,"message":message,"data":{"kind":kind,"stage":stage,"message":message}}
    })
}

pub(crate) fn not_found(id: Value, stage: &str) -> Value {
    failure(
        id,
        -32050,
        "notFound",
        stage,
        "not found (expired after 30 days, or never existed)",
    )
}
