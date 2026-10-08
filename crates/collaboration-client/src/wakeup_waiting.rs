//! First-fire waiting: bounded waits chained by their resume cursor; ordinary calls keep their
//! 30-second deadline.
use crate::api_connection::ToolAnswer;
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
    FireReceipt, UuidIdentity, WakeShowRequest, WakeWaitOutcome, WakeWaitRequest, WakeWaitResult,
    WakeupId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
pub enum WakeWaitFailureKind {
    #[serde(rename = "wakeNotFound")]
    NotFound,
    #[serde(rename = "waitUnavailable")]
    Unavailable,
    #[serde(rename = "wakePaused")]
    Paused,
    #[serde(rename = "wakeCancelled")]
    Cancelled,
    #[serde(rename = "wakeExpired")]
    Expired,
    #[serde(rename = "wakeFinishedWithoutFiring")]
    FinishedWithoutFiring,
    #[serde(rename = "connectionUnavailable")]
    Connection,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeWaitFailure {
    pub kind: WakeWaitFailureKind,
    pub stage: String,
    pub effect: crate::OperationEffect,
    pub message: String,
    pub wakeup_id: Option<WakeupId>,
    pub first_occurrence_id: Option<String>,
    pub next_action: String,
    pub effects: WakeWaitEffects,
    pub connection: Option<crate::OperationFailure>,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeWaitEffects {
    pub first_fire: String,
}
#[derive(Debug, thiserror::Error)]
pub enum WakeWaitError {
    #[error(
        "Wake-up was not found in this service; verify its address. Historical firing is unknown."
    )]
    NotFound { wakeup_id: WakeupId },
    #[error("Wake wait unavailable; reconnect the wait without recreating the reminder.")]
    Unavailable { wakeup_id: WakeupId },
    #[error("Wake-up paused before its first firing; resume it before starting a new wait.")]
    Paused { wakeup_id: WakeupId },
    #[error("Wake-up cancelled before its first firing; create another reminder if needed.")]
    Cancelled { wakeup_id: WakeupId },
    #[error("Wake-up expired before its first firing.")]
    Expired { wakeup_id: WakeupId },
    #[error("Wake-up has no future firing; its one-shot tick was skipped while paused.")]
    FinishedWithoutFiring { wakeup_id: WakeupId },
    #[error("Wake-up first-fire wait was cancelled by its caller.")]
    CallerCancelled { wakeup_id: WakeupId },
    #[error(transparent)]
    Connection(#[from] ClientError),
}

impl WakeWaitError {
    #[must_use]
    pub fn into_operation_failure(self) -> WakeWaitFailure {
        let message = self.to_string();
        let (kind, stage, wakeup_id, next_action, first_fire, connection) = match self {
            Self::NotFound { wakeup_id } => (
                WakeWaitFailureKind::NotFound,
                "subscribe",
                Some(wakeup_id),
                "verifyWakeupAddress",
                "unknown",
                None,
            ),
            Self::Unavailable { wakeup_id } => (
                WakeWaitFailureKind::Unavailable,
                "wait",
                Some(wakeup_id),
                "reconnectWait",
                "unknown",
                None,
            ),
            Self::Paused { wakeup_id } => (
                WakeWaitFailureKind::Paused,
                "wait",
                Some(wakeup_id),
                "resumeWakeup",
                "notRecorded",
                None,
            ),
            Self::Cancelled { wakeup_id } => (
                WakeWaitFailureKind::Cancelled,
                "wait",
                Some(wakeup_id),
                "createWakeup",
                "notRecorded",
                None,
            ),
            Self::Expired { wakeup_id } => (
                WakeWaitFailureKind::Expired,
                "wait",
                Some(wakeup_id),
                "createWakeup",
                "notRecorded",
                None,
            ),
            Self::FinishedWithoutFiring { wakeup_id } => (
                WakeWaitFailureKind::FinishedWithoutFiring,
                "wait",
                Some(wakeup_id),
                "createWakeup",
                "notRecorded",
                None,
            ),
            Self::CallerCancelled { wakeup_id } => (
                WakeWaitFailureKind::Unavailable,
                "wait",
                Some(wakeup_id),
                "reconnectWait",
                "unknown",
                None,
            ),
            Self::Connection(error) => (
                WakeWaitFailureKind::Connection,
                "connection",
                None,
                "reconnectWait",
                "unknown",
                Some(crate::operation_failure_from_client_error(
                    error,
                    crate::OperationEffect::None,
                )),
            ),
        };
        WakeWaitFailure {
            kind,
            stage: stage.to_owned(),
            effect: crate::OperationEffect::None,
            message,
            wakeup_id,
            first_occurrence_id: None,
            next_action: next_action.to_owned(),
            effects: WakeWaitEffects {
                first_fire: first_fire.to_owned(),
            },
            connection,
        }
    }
}
/// A wait for one wake-up's first fire, made of bounded waits that each resume from the
/// cursor the previous one returned, so no change between them is missed.
pub struct WakeWaitConnection {
    client: CollaborationClient,
    wakeup_id: WakeupId,
}

/// The longest one bounded wait lasts before it returns its cursor.
const WAKE_WAIT_CALL_SECONDS: u32 = 1500;

impl CollaborationClient {
    pub async fn subscribe_wakeup(
        self,
        request: WakeShowRequest,
    ) -> Result<WakeWaitConnection, WakeWaitError> {
        Ok(WakeWaitConnection {
            client: self,
            wakeup_id: request.wakeup_id,
        })
    }
}

impl WakeWaitConnection {
    pub async fn wait_until_first_fire(self) -> Result<FireReceipt, WakeWaitError> {
        self.wait_until_first_fire_with_cancellation(CancellationToken::new())
            .await
    }

    pub async fn wait_until_first_fire_with_cancellation(
        self,
        cancellation: CancellationToken,
    ) -> Result<FireReceipt, WakeWaitError> {
        let id = self.wakeup_id.clone();
        let mut after = None;
        loop {
            let request = WakeWaitRequest {
                wakeup_id: id.clone(),
                after: after.clone(),
                timeout_seconds: Some(WAKE_WAIT_CALL_SECONDS),
            };
            let arguments = serde_json::to_value(&request)
                .map_err(|_| ClientError::Protocol("invalid wake wait"))?;
            let answer = tokio::select! {
                () = cancellation.cancelled() => {
                    return Err(WakeWaitError::CallerCancelled { wakeup_id: id });
                }
                answer = self.client.connection.answer_with_timeout(
                    "wake_wait_until_first_fire",
                    arguments,
                    Duration::from_secs(u64::from(WAKE_WAIT_CALL_SECONDS) + 30),
                ) => answer?,
            };
            let waited: WakeWaitResult = match answer {
                ToolAnswer::Success(value) => serde_json::from_value(value)
                    .map_err(|_| ClientError::Protocol("invalid wake wait result"))?,
                ToolAnswer::Failure(failure) => return Err(wait_failure(&id, failure)),
            };
            if waited.wakeup_id != id {
                return Err(ClientError::Protocol("wake wait identity mismatch").into());
            }
            cursor_sequence(&waited.cursor, &self.client.identity().service_id)?;
            match waited.outcome {
                WakeWaitOutcome::Fired { fire } => {
                    if fire.wakeup_id != id {
                        return Err(ClientError::Protocol("firing identity mismatch").into());
                    }
                    return Ok(fire);
                }
                WakeWaitOutcome::Paused => return Err(WakeWaitError::Paused { wakeup_id: id }),
                WakeWaitOutcome::Cancelled => {
                    return Err(WakeWaitError::Cancelled { wakeup_id: id });
                }
                WakeWaitOutcome::Expired => return Err(WakeWaitError::Expired { wakeup_id: id }),
                WakeWaitOutcome::FinishedWithoutFiring => {
                    return Err(WakeWaitError::FinishedWithoutFiring { wakeup_id: id });
                }
                WakeWaitOutcome::TimedOut => after = Some(waited.cursor),
            }
        }
    }
}

/// The wait error a refused wait reports.
fn wait_failure(id: &WakeupId, mut failure: Value) -> WakeWaitError {
    if let Some(fields) = failure.as_object_mut() {
        fields.remove("mcpResult");
    }
    match serde_json::from_value::<WakeWaitFailure>(failure.clone()) {
        Ok(typed) if typed.wakeup_id.as_ref() == Some(id) => match typed.kind {
            WakeWaitFailureKind::NotFound => WakeWaitError::NotFound {
                wakeup_id: id.clone(),
            },
            WakeWaitFailureKind::Paused => WakeWaitError::Paused {
                wakeup_id: id.clone(),
            },
            WakeWaitFailureKind::Cancelled => WakeWaitError::Cancelled {
                wakeup_id: id.clone(),
            },
            WakeWaitFailureKind::Expired => WakeWaitError::Expired {
                wakeup_id: id.clone(),
            },
            WakeWaitFailureKind::FinishedWithoutFiring => WakeWaitError::FinishedWithoutFiring {
                wakeup_id: id.clone(),
            },
            WakeWaitFailureKind::Unavailable | WakeWaitFailureKind::Connection => {
                WakeWaitError::Unavailable {
                    wakeup_id: id.clone(),
                }
            }
        },
        _ => crate::api_connection::rejection_from_tool_failure(failure).into(),
    }
}

fn cursor_sequence(cursor: &str, service_id: &UuidIdentity) -> Result<i64, ClientError> {
    let (version, service, collection, sequence, at): (u8, UuidIdentity, String, i64, i64) =
        serde_json::from_str(cursor)
            .map_err(|_| ClientError::Protocol("invalid wake event cursor"))?;
    if version != 1
        || &service != service_id
        || collection != "automation-events"
        || sequence < 0
        || at < 0
    {
        return Err(ClientError::Protocol("wake cursor scope mismatch"));
    }
    Ok(sequence)
}

#[cfg(test)]
mod tests {
    use super::{WakeWaitError, WakeWaitFailureKind};

    #[test]
    fn shared_wake_failure_preserves_effect_and_recovery_action() {
        let wakeup_id: collaboration_protocol::WakeupId = "01a0bb62-9a72-7161-8103-b6c2c691bec8"
            .to_owned()
            .try_into()
            .expect("wake ID");
        let paused = WakeWaitError::Paused {
            wakeup_id: wakeup_id.clone(),
        }
        .into_operation_failure();
        assert_eq!(paused.kind, WakeWaitFailureKind::Paused);
        assert_eq!(paused.wakeup_id, Some(wakeup_id));
        assert_eq!(paused.next_action, "resumeWakeup");
        assert_eq!(paused.effects.first_fire, "notRecorded");
        assert_eq!(paused.effect, crate::OperationEffect::None);
    }
}
