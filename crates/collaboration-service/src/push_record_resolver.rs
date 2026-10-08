//! Resolves local push links, applies reported-caller access rules, and expands stored ranges.
//! The `show`, `inbox` and `history` Control entry points decode requests for the typed
//! message operations.
use crate::ServiceIdentity;
use crate::collaboration_application::{MessageFailure, MessageFailureKind, MessageOperations};
use collaboration_protocol::{
    MachineId, PushActivityRangeRead, PushId, PushKind, PushLineInput, PushMessageSendResult,
    PushOrigin, PushRecord, PushRecordHistoryParams, PushRecordListParams, PushRecordNotice,
    PushRecordShowParams, RouterLink, SessionRef, render_push_line,
};
use message_board::{
    MessageListRequest, MessageListScope, MessageSelection, PageLimit, PageRequest,
};
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
        delivery_state: record.delivery_state,
        receipt: record
            .last_outcome
            .clone()
            .ok_or(collaboration_protocol::PushLineError::InvalidHeaderFacts)?,
    })
}

pub(crate) async fn show(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<PushRecordShowParams>(params) else {
        return failure(
            id,
            -32602,
            "invalidField",
            "inspect",
            "Invalid push show request",
        );
    };
    message_response(id, MessageOperations::new(identity).push_show(params).await)
}

pub(crate) async fn inbox(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<PushRecordListParams>(params) else {
        return failure(
            id,
            -32602,
            "invalidField",
            "discovery",
            "Invalid message inbox request",
        );
    };
    message_response(
        id,
        MessageOperations::new(identity).message_inbox(params).await,
    )
}

pub(crate) async fn history(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<PushRecordHistoryParams>(params) else {
        return failure(
            id,
            -32602,
            "invalidField",
            "discovery",
            "Invalid message history request",
        );
    };
    message_response(
        id,
        MessageOperations::new(identity)
            .message_history(params)
            .await,
    )
}

/// Control encodes a refused request field or service as invalid params, other failures as -32050.
pub(crate) fn message_response(
    id: Value,
    result: Result<impl serde::Serialize, MessageFailure>,
) -> Value {
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(failure) => {
            let code = match failure.kind {
                MessageFailureKind::InvalidField | MessageFailureKind::WrongService => -32602,
                MessageFailureKind::ForeignMachine
                | MessageFailureKind::Unavailable
                | MessageFailureKind::NotFound
                | MessageFailureKind::NotPermitted
                | MessageFailureKind::NotDirectMessage
                | MessageFailureKind::OwnerReplyUnsupported
                | MessageFailureKind::OutcomeUnknown => -32050,
            };
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":failure.message,"data":failure}})
        }
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
        return Err(ReferenceError::Foreign(
            link.machine_id().as_str().to_owned(),
        ));
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

pub(crate) fn caller_is_local(caller: &SessionRef, identity: &ServiceIdentity) -> bool {
    caller.endpoint.service_id == identity.service_id
}

pub(crate) fn make_notice_list(
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

pub(crate) enum ActivityReadError {
    Unavailable,
    Failed,
}

pub(crate) async fn expand_activity_ranges(
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

pub(crate) fn failure(id: Value, code: i64, kind: &str, stage: &str, message: &str) -> Value {
    json!({
        "jsonrpc":"2.0",
        "id":id,
        "error":{"code":code,"message":message,"data":{"kind":kind,"stage":stage,"message":message}}
    })
}
