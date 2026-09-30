//! Decode durable subscription rows through private SQLx shapes and domain constructors.
use crate::storage_support::{StoredIdentityRow, decode_identity};
use chrono::{DateTime, Utc};
use message_board::*;

pub(crate) struct StoredThreadSubscriptionRow {
    pub reader_key: String,
    pub reader_kind: String,
    pub service_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub session_id: Option<String>,
    pub human_id: Option<String>,
    pub scope_kind: String,
    pub scope_id: String,
    pub mode: String,
    pub when_idle: String,
    pub quiet_seconds: i64,
    pub cap_seconds: i64,
    pub lifetime_seconds: i64,
    pub renewed_at: String,
    pub expires_at: String,
    pub state: String,
    pub end_reason: Option<String>,
    pub ended_at: Option<String>,
    pub last_outcome: Option<String>,
    pub generation: i64,
}

pub(crate) struct StoredSubscriptionWindowRow {
    pub root_id: String,
    pub opened_at: String,
    pub last_arrival_at: String,
    pub pending_count: i64,
    pub held_since: Option<String>,
    pub retry_not_before: Option<String>,
    pub retry_attempts: i64,
    pub window_id: String,
}

pub(crate) fn decode_thread_subscription_record(
    row: StoredThreadSubscriptionRow,
    windows: Vec<StoredSubscriptionWindowRow>,
) -> Result<ThreadSubscriptionRecord, BoardError> {
    let reader = decode_identity(&StoredIdentityRow {
        identity_key: row.reader_key,
        kind: row.reader_kind,
        service_id: row.service_id,
        endpoint_id: row.endpoint_id,
        session_id: row.session_id,
        human_id: row.human_id,
    })
    .map_err(|_| corrupt_subscription("reader"))?;
    let scope = decode_scope(row.scope_kind.as_str(), row.scope_id)?;
    let mode = decode_mode(row.mode.as_str())?;
    let when_idle = decode_when_idle(row.when_idle.as_str())?;
    let quiet_seconds =
        u64::try_from(row.quiet_seconds).map_err(|_| corrupt_subscription("quietSeconds"))?;
    let cap_seconds =
        u64::try_from(row.cap_seconds).map_err(|_| corrupt_subscription("capSeconds"))?;
    let lifetime_seconds =
        u64::try_from(row.lifetime_seconds).map_err(|_| corrupt_subscription("lifetimeSeconds"))?;
    let timing = BatchTiming::new(quiet_seconds, cap_seconds)
        .map_err(|error| corrupt_subscription(error.field))?;
    let lifetime = SubscriptionLifetime::new(lifetime_seconds)
        .map_err(|error| corrupt_subscription(error.field))?;
    let policy = SubscriptionPolicy::new(&reader, mode, when_idle, timing, lifetime)
        .map_err(|error| corrupt_subscription(error.field))?;
    let (state, ended_at) = decode_state(
        row.state.as_str(),
        row.end_reason.as_deref(),
        row.ended_at.as_deref(),
    )?;
    let last_outcome = row
        .last_outcome
        .map(|value| {
            serde_json::from_str::<SubscriptionDeliveryOutcome>(value.as_str())
                .map_err(|_| corrupt_subscription("lastOutcome"))
        })
        .transpose()?;
    let windows = windows
        .into_iter()
        .map(decode_subscription_root_record)
        .collect::<Result<Vec<_>, _>>()?;
    let generation_value =
        u64::try_from(row.generation).map_err(|_| corrupt_subscription("generation"))?;
    let generation = SubscriptionGeneration::new(generation_value)
        .map_err(|error| corrupt_subscription(error.field))?;
    ThreadSubscriptionRecord::new(ThreadSubscriptionRecordProps {
        reader,
        scope,
        policy,
        state,
        renewed_at: decode_utc_timestamp("renewedAt", row.renewed_at.as_str())?,
        expires_at: decode_utc_timestamp("expiresAt", row.expires_at.as_str())?,
        ended_at,
        generation,
        last_outcome,
        roots: windows,
    })
    .map_err(|error| corrupt_subscription(error.field))
}

fn decode_subscription_root_record(
    row: StoredSubscriptionWindowRow,
) -> Result<SubscriptionRootRecord, BoardError> {
    let root_message_id =
        MessageId::try_from(row.root_id).map_err(|_| corrupt_subscription("rootId"))?;
    let opened_at = decode_utc_timestamp("openedAt", row.opened_at.as_str())?;
    let last_arrival_at = decode_utc_timestamp("lastArrivalAt", row.last_arrival_at.as_str())?;
    let pending_count =
        u64::try_from(row.pending_count).map_err(|_| corrupt_subscription("pendingCount"))?;
    let held_since = row
        .held_since
        .map(|value| decode_utc_timestamp("heldSince", value.as_str()))
        .transpose()?;
    let next_retry_at = row
        .retry_not_before
        .map(|value| decode_utc_timestamp("nextRetryAt", value.as_str()))
        .transpose()?;
    let retry_attempts =
        u32::try_from(row.retry_attempts).map_err(|_| corrupt_subscription("retryAttempts"))?;
    SubscriptionWindowId::try_from(row.window_id).map_err(|_| corrupt_subscription("windowId"))?;
    SubscriptionRootRecord::new(SubscriptionRootRecordProps {
        root_message_id,
        opened_at,
        last_arrival_at,
        pending_count,
        held_since,
        next_retry_at,
        retry_attempts,
    })
    .map_err(|error| corrupt_subscription(error.field))
}

fn decode_scope(scope_kind: &str, scope_id: String) -> Result<SubscriptionScope, BoardError> {
    match scope_kind {
        "thread" => MessageId::try_from(scope_id)
            .map(SubscriptionScope::thread)
            .map_err(|_| corrupt_subscription("scopeId")),
        "topic" => TopicId::try_from(scope_id)
            .map(SubscriptionScope::topic)
            .map_err(|_| corrupt_subscription("scopeId")),
        _ => Err(corrupt_subscription("scopeKind")),
    }
}

fn decode_mode(value: &str) -> Result<SubscriptionMode, BoardError> {
    match value {
        "deliver" => Ok(SubscriptionMode::Deliver),
        "poll" => Ok(SubscriptionMode::Poll),
        "off" => Ok(SubscriptionMode::Off),
        _ => Err(corrupt_subscription("mode")),
    }
}

fn decode_when_idle(value: &str) -> Result<WhenIdle, BoardError> {
    match value {
        "hold" => Ok(WhenIdle::Hold),
        "wake" => Ok(WhenIdle::Wake),
        "drop" => Ok(WhenIdle::Drop),
        _ => Err(corrupt_subscription("whenIdle")),
    }
}

fn decode_state(
    state: &str,
    reason: Option<&str>,
    ended_at: Option<&str>,
) -> Result<(SubscriptionState, Option<DateTime<Utc>>), BoardError> {
    match (state, reason, ended_at) {
        ("active", None, None) => Ok((SubscriptionState::Active, None)),
        ("draining", None, None) => Ok((SubscriptionState::Draining, None)),
        ("ended", Some(reason), Some(ended_at)) => {
            let reason = match reason {
                "left" => EndReason::Left,
                "replaced" => EndReason::Replaced,
                "resolved" => EndReason::Resolved,
                "cancelled" => EndReason::Cancelled,
                "expired" => EndReason::Expired,
                _ => return Err(corrupt_subscription("endReason")),
            };
            Ok((
                SubscriptionState::Ended { reason },
                Some(decode_utc_timestamp("endedAt", ended_at)?),
            ))
        }
        ("ended", Some(_), None) => Err(corrupt_subscription("endedAt")),
        ("ended", None, _) => Err(corrupt_subscription("endReason")),
        ("active" | "draining", Some(_), _) => Err(corrupt_subscription("endReason")),
        ("active" | "draining", None, Some(_)) => Err(corrupt_subscription("endedAt")),
        _ => Err(corrupt_subscription("state")),
    }
}

pub(crate) fn decode_utc_timestamp(
    field: &'static str,
    value: &str,
) -> Result<DateTime<Utc>, BoardError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| corrupt_subscription(field))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(corrupt_subscription(field));
    }
    Ok(parsed.with_timezone(&Utc))
}

pub(crate) fn encode_utc_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn corrupt_subscription(field: &str) -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidRecord,
        stage: BoardFailureStage::Inspection,
        message: format!("Stored thread subscription field '{field}' is invalid."),
        next_action: BoardNextAction::InspectResource,
        details: BoardErrorDetails::FieldConstraint {
            field: format!("threadSubscription.{field}"),
            requirement: "must contain a valid stored value".to_owned(),
        },
    }
}
