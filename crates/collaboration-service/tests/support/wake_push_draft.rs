use agent_automation::{FirstFire, WakeRecord};
use automation_storage::{PushRecordDraft, StorageError};
use collaboration_protocol::{
    MessageContent, PushHeaderFacts, PushId, PushKind, PushOrigin, RouterOriginRef, SavedMessage,
};

pub(super) fn build_test_wake_push_draft(
    wake: &WakeRecord<SavedMessage>,
    fire: &FirstFire,
) -> Result<PushRecordDraft, StorageError> {
    if fire.wakeup_id != wake.definition.wakeup_id {
        return Err(StorageError::InvalidRecord);
    }
    let body = match &wake.definition.message.content {
        MessageContent::Agent { text, .. }
        | MessageContent::HumanUser { text }
        | MessageContent::Router { text } => text.as_str().to_owned(),
    };
    let origin_router_ref = RouterOriginRef::Wake {
        wakeup_id: fire.wakeup_id.clone(),
        occurrence_id: fire.occurrence_id.clone(),
    }
    .canonical_string()
    .map_err(|_| StorageError::InvalidRecord)?;
    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string())
        .map_err(|_| StorageError::InvalidRecord)?;
    let created_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(fire.fired_at_ms)
        .ok_or(StorageError::InvalidRecord)?;
    Ok(PushRecordDraft {
        push_id,
        kind: PushKind::Wake,
        origin: PushOrigin::Router(PushKind::Wake),
        origin_router_ref: Some(origin_router_ref),
        target: wake.definition.message.target.clone(),
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::Wake,
        body: Some(body),
        activity: None,
        created_at,
    })
}
