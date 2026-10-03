use agent_automation::{DurableMessage, FirstFire, WakeRecord};
use automation_storage::{PushRecordDraft, StorageError};
use collaboration_protocol::{
    EndpointId, EndpointRef, PushHeaderFacts, PushId, PushKind, PushOrigin, RouterOriginRef,
    SessionId, SessionRef, UuidIdentity,
};
use serde_json::Value;

pub(super) fn build_test_wake_push_draft<TMessage: DurableMessage>(
    wake: &WakeRecord<TMessage>,
    fire: &FirstFire,
) -> Result<PushRecordDraft, StorageError> {
    if fire.wakeup_id != wake.definition.wakeup_id {
        return Err(StorageError::InvalidRecord);
    }
    let target_value = serde_json::to_value(wake.definition.message.target())
        .map_err(|_| StorageError::InvalidRecord)?;
    let target = match serde_json::from_value::<SessionRef>(target_value) {
        Ok(target) => target,
        Err(_) => fixture_target()?,
    };
    let content = serde_json::to_value(wake.definition.message.content())
        .map_err(|_| StorageError::InvalidRecord)?;
    let body = match content {
        Value::String(text) => text,
        value => value.to_string(),
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
        mode: None,
        guard: None,
        push_id,
        kind: PushKind::Wake,
        origin: PushOrigin::Router(PushKind::Wake),
        origin_router_ref: Some(origin_router_ref),
        target,
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::Wake,
        body: Some(body),
        activity: None,
        created_at,
    })
}

fn fixture_target() -> Result<SessionRef, StorageError> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())
                .map_err(|_| StorageError::InvalidRecord)?,
            endpoint_id: EndpointId::try_from("codex-local".to_owned())
                .map_err(|_| StorageError::InvalidRecord)?,
        },
        session_id: SessionId::try_from("storage-test-wake".to_owned())
            .map_err(|_| StorageError::InvalidRecord)?,
    })
}
