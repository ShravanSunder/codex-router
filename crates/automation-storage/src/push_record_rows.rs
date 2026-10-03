//! Typed conversion between persisted push columns and validated protocol records.
use crate::StorageError;
use chrono::{DateTime, Utc};
use collaboration_protocol::{
    EndpointId, EndpointRef, MessageDelivery, PushActivitySnapshot, PushDeliveryState, PushId,
    PushKind, PushOrigin, PushRecord, PushRecordDraft, RouterOriginRef, SessionId, SessionRef,
    UuidIdentity,
};
use serde::Serialize;
use sqlx::FromRow;

#[derive(FromRow)]
pub(crate) struct PushRecordRow {
    pub push_id: String,
    pub kind: String,
    pub origin_kind: String,
    pub origin_service_id: Option<String>,
    pub origin_endpoint_id: Option<String>,
    pub origin_session_id: Option<String>,
    pub origin_router_ref: Option<String>,
    pub target_service_id: String,
    pub target_endpoint_id: String,
    pub target_session_id: String,
    pub dm_delivery_mode: Option<String>,
    pub dm_generation_guard_json: Option<String>,
    pub reply_to_push_id: Option<String>,
    pub header_facts_json: String,
    pub body: Option<String>,
    pub ranges_json: Option<String>,
    pub delivery_state: String,
    pub last_outcome_json: Option<String>,
    pub created_at: String,
    pub settled_at: Option<String>,
    pub read_at: Option<String>,
}

impl PushRecordRow {
    pub(crate) fn into_record(self) -> Result<PushRecord, StorageError> {
        let kind = decode_json_enum(&self.kind)?;
        let origin = match self.origin_kind.as_str() {
            "session" => {
                if self.origin_router_ref.is_some() {
                    return Err(StorageError::InvalidRecord);
                }
                PushOrigin::Session(session_from_parts(
                    self.origin_service_id.ok_or(StorageError::InvalidRecord)?,
                    self.origin_endpoint_id.ok_or(StorageError::InvalidRecord)?,
                    self.origin_session_id.ok_or(StorageError::InvalidRecord)?,
                )?)
            }
            "owner" => {
                if self.origin_service_id.is_some()
                    || self.origin_endpoint_id.is_some()
                    || self.origin_session_id.is_some()
                    || self.origin_router_ref.is_some()
                {
                    return Err(StorageError::InvalidRecord);
                }
                PushOrigin::OwnerUnverified
            }
            "router" => {
                if self.origin_service_id.is_some()
                    || self.origin_endpoint_id.is_some()
                    || self.origin_session_id.is_some()
                {
                    return Err(StorageError::InvalidRecord);
                }
                let origin = PushOrigin::Router(kind);
                validate_router_origin_reference(kind, &origin, self.origin_router_ref.as_deref())?;
                origin
            }
            _ => return Err(StorageError::InvalidRecord),
        };
        let target = session_from_parts(
            self.target_service_id,
            self.target_endpoint_id,
            self.target_session_id,
        )?;
        let push_id = PushId::try_from(self.push_id).map_err(|_| StorageError::InvalidRecord)?;
        let reply_to_push_id = self
            .reply_to_push_id
            .map(PushId::try_from)
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?;
        let header_facts = serde_json::from_str(&self.header_facts_json)
            .map_err(|_| StorageError::InvalidRecord)?;
        let activity = self
            .ranges_json
            .map(|json| serde_json::from_str::<PushActivitySnapshot>(&json))
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?;
        let last_outcome = self
            .last_outcome_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?;
        let delivery_state = decode_delivery_state(&self.delivery_state)?;
        let mode = self
            .dm_delivery_mode
            .as_deref()
            .map(decode_direct_message_mode)
            .transpose()?;
        let guard = self
            .dm_generation_guard_json
            .as_deref()
            .map(serde_json::from_str::<collaboration_protocol::CodexGeneration>)
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?;
        let draft = PushRecordDraft {
            mode,
            guard,
            push_id,
            kind,
            origin,
            origin_router_ref: self.origin_router_ref,
            target,
            reply_to_push_id,
            header_facts,
            body: self.body,
            activity,
            created_at: decode_timestamp(self.created_at)?,
        };
        PushRecord::from_storage_parts(
            draft,
            delivery_state,
            last_outcome,
            self.settled_at.map(decode_timestamp).transpose()?,
            self.read_at.map(decode_timestamp).transpose()?,
        )
        .map_err(|_| StorageError::InvalidRecord)
    }
}

pub(crate) fn validate_router_origin_reference(
    kind: PushKind,
    origin: &PushOrigin,
    encoded_origin_reference: Option<&str>,
) -> Result<(), StorageError> {
    let origin_kind = match origin {
        PushOrigin::Router(origin_kind) if *origin_kind == kind => *origin_kind,
        PushOrigin::Router(_) => return Err(StorageError::InvalidRecord),
        PushOrigin::Session(_) | PushOrigin::OwnerUnverified
            if encoded_origin_reference.is_none() =>
        {
            return Ok(());
        }
        PushOrigin::Session(_) | PushOrigin::OwnerUnverified => {
            return Err(StorageError::InvalidRecord);
        }
    };
    let encoded_origin_reference = encoded_origin_reference.ok_or(StorageError::InvalidRecord)?;
    let origin_reference = RouterOriginRef::parse_canonical(encoded_origin_reference)
        .map_err(|_| StorageError::InvalidRecord)?;
    if !origin_reference.supports_kind(origin_kind) {
        return Err(StorageError::InvalidRecord);
    }
    Ok(())
}

pub(crate) fn serialize_json<TValue: Serialize>(value: &TValue) -> Result<String, StorageError> {
    serde_json::to_string(value).map_err(|_| StorageError::InvalidRecord)
}

pub(crate) fn serialize_kind(kind: PushKind) -> Result<String, StorageError> {
    serde_json::to_value(kind)
        .map_err(|_| StorageError::InvalidRecord)?
        .as_str()
        .map(str::to_owned)
        .ok_or(StorageError::InvalidRecord)
}

pub(crate) fn serialize_origin(origin: &PushOrigin) -> (&'static str, Option<&SessionRef>) {
    match origin {
        PushOrigin::Session(session) => ("session", Some(session)),
        PushOrigin::OwnerUnverified => ("owner", None),
        PushOrigin::Router(_) => ("router", None),
    }
}

pub(crate) fn serialize_delivery_state(state: PushDeliveryState) -> &'static str {
    match state {
        PushDeliveryState::Pending => "pending",
        PushDeliveryState::Attempted => "attempted",
        PushDeliveryState::Held => "held",
        PushDeliveryState::Delivered => "delivered",
        PushDeliveryState::OutcomeUnknown => "outcome_unknown",
        PushDeliveryState::Rejected => "rejected",
    }
}

pub(crate) fn serialize_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn decode_delivery_state(value: &str) -> Result<PushDeliveryState, StorageError> {
    match value {
        "pending" => Ok(PushDeliveryState::Pending),
        "attempted" => Ok(PushDeliveryState::Attempted),
        "held" => Ok(PushDeliveryState::Held),
        "delivered" => Ok(PushDeliveryState::Delivered),
        "outcome_unknown" => Ok(PushDeliveryState::OutcomeUnknown),
        "rejected" => Ok(PushDeliveryState::Rejected),
        _ => Err(StorageError::InvalidRecord),
    }
}

fn decode_timestamp(value: String) -> Result<DateTime<Utc>, StorageError> {
    if !value.ends_with('Z') {
        return Err(StorageError::InvalidRecord);
    }
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| StorageError::InvalidRecord)
}

fn decode_json_enum<TEnum: serde::de::DeserializeOwned>(
    value: &str,
) -> Result<TEnum, StorageError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| StorageError::InvalidRecord)
}

pub(crate) fn session_from_parts(
    service_id: String,
    endpoint_id: String,
    session_id: String,
) -> Result<SessionRef, StorageError> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from(service_id)
                .map_err(|_| StorageError::InvalidRecord)?,
            endpoint_id: EndpointId::try_from(endpoint_id)
                .map_err(|_| StorageError::InvalidRecord)?,
        },
        session_id: SessionId::try_from(session_id).map_err(|_| StorageError::InvalidRecord)?,
    })
}

fn decode_direct_message_mode(value: &str) -> Result<MessageDelivery, StorageError> {
    match value {
        "auto" => Ok(MessageDelivery::Auto),
        "queue" => Ok(MessageDelivery::Queue),
        "steer" => Ok(MessageDelivery::Steer),
        _ => Err(StorageError::InvalidRecord),
    }
}

pub(crate) fn serialize_direct_message_mode(mode: Option<MessageDelivery>) -> Option<&'static str> {
    mode.map(|mode| match mode {
        MessageDelivery::Auto => "auto",
        MessageDelivery::Queue => "queue",
        MessageDelivery::Steer => "steer",
    })
}
