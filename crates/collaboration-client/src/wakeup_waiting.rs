//! First-fire waiting consumes a dedicated initialized client; ordinary calls keep their 30-second deadline.
use crate::{ClientError, ControlClient};
use collaboration_protocol::WakeupId;
use collaboration_protocol::{
    FireReceipt, WakeChange, WakeChanged, WakeShowRequest, WakeState, WakeSubscription,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
            Self::Connection(error) => (
                WakeWaitFailureKind::Connection,
                "connection",
                None,
                "reconnectWait",
                "unknown",
                Some(crate::OperationFailure::from_client_error(
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
pub struct WakeWaitConnection {
    client: ControlClient,
    subscription: WakeSubscription,
}
impl ControlClient {
    pub async fn subscribe_wakeup(
        mut self,
        request: WakeShowRequest,
    ) -> Result<WakeWaitConnection, WakeWaitError> {
        let result = match self
            .connection
            .call(
                "wake/subscribe",
                serde_json::to_value(&request)
                    .map_err(|_| ClientError::Protocol("invalid wake subscription"))?,
            )
            .await
        {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                if let Ok(error) =
                    serde_json::from_value::<collaboration_protocol::WakeNotFound>(data.clone())
                {
                    if error.wakeup_id != request.wakeup_id {
                        return Err(ClientError::Protocol(
                            "missing wake response identity mismatch",
                        )
                        .into());
                    }
                    return Err(WakeWaitError::NotFound {
                        wakeup_id: error.wakeup_id,
                    });
                }
                let error: collaboration_protocol::WaitUnavailable =
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid wait failure"))?;
                if error.wakeup_id != request.wakeup_id {
                    return Err(ClientError::Protocol("wait failure identity mismatch").into());
                }
                return Err(WakeWaitError::Unavailable {
                    wakeup_id: error.wakeup_id,
                });
            }
            Err(error) => return Err(error.into()),
        };
        let subscription: WakeSubscription = serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid wake subscription response"))?;
        if subscription.snapshot.definition.wakeup_id != request.wakeup_id {
            return Err(ClientError::Protocol("wake subscription identity mismatch").into());
        }
        Ok(WakeWaitConnection {
            client: self,
            subscription,
        })
    }
}
impl WakeWaitConnection {
    pub fn subscription(&self) -> &WakeSubscription {
        &self.subscription
    }
    pub async fn wait_until_first_fire(mut self) -> Result<FireReceipt, WakeWaitError> {
        let id = self.subscription.snapshot.definition.wakeup_id.clone();
        if let Some(fire) = self.subscription.snapshot.first_fire.take() {
            return Ok(fire);
        }
        match self.subscription.snapshot.state {
            WakeState::Paused => return Err(WakeWaitError::Paused { wakeup_id: id }),
            WakeState::Cancelled => return Err(WakeWaitError::Cancelled { wakeup_id: id }),
            WakeState::Expired => return Err(WakeWaitError::Expired { wakeup_id: id }),
            WakeState::Finished => {
                return Err(WakeWaitError::FinishedWithoutFiring { wakeup_id: id });
            }
            WakeState::Active => {}
        }
        let previous = cursor_sequence(&self.subscription.after, self.client.identity())?;
        {
            let frame = self.client.next_wake_notification().await?;
            let changed: WakeChanged = serde_json::from_value(frame.get("params").cloned().ok_or(
                ClientError::Protocol("wake notification parameters missing"),
            )?)
            .map_err(|_| ClientError::Protocol("invalid wake notification"))?;
            let sequence = cursor_sequence(&changed.cursor, self.client.identity())?;
            if sequence <= previous
                || changed.subscription_id != self.subscription.subscription_id
                || changed.wakeup_id != id
            {
                return Err(ClientError::Protocol(
                    "wake notification identity or sequence mismatch",
                )
                .into());
            }
            match changed.change {
                WakeChange::Fired { fire } => {
                    if fire.wakeup_id != id {
                        return Err(ClientError::Protocol("firing identity mismatch").into());
                    }
                    Ok(fire)
                }
                WakeChange::Paused => Err(WakeWaitError::Paused { wakeup_id: id }),
                WakeChange::Cancelled => Err(WakeWaitError::Cancelled { wakeup_id: id }),
                WakeChange::Expired => Err(WakeWaitError::Expired { wakeup_id: id }),
                WakeChange::FinishedWithoutFiring => {
                    Err(WakeWaitError::FinishedWithoutFiring { wakeup_id: id })
                }
            }
        }
    }
}
fn cursor_sequence(
    cursor: &str,
    identity: &collaboration_protocol::ControlInitializationResult,
) -> Result<i64, ClientError> {
    let (version, service, collection, sequence, at): (
        u8,
        collaboration_protocol::UuidIdentity,
        String,
        i64,
        i64,
    ) = serde_json::from_str(cursor)
        .map_err(|_| ClientError::Protocol("invalid wake event cursor"))?;
    if version != 1
        || service != identity.service_id
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
