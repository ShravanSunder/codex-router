//! Wakes and their deliveries: durable reminders in automation storage. Timing workers own the
//! later native submission; these operations never touch a native thread.
use super::{
    CollaborationRejection, CollaborationRejectionReason, PublishedRejection, ResultByteBudget,
};
use crate::wakeup_subscription::{WakeResumeError, WakeSubscriptionState};
use agent_automation::WakeupId;
use automation_storage::{
    AutomationStore, StorageError, WakeAction, WakeCreate, WakeListPosition, WakeMutation,
};
use collaboration_protocol::{
    AutomationPage, AutomationPageRequest, DeliveryInspection, DeliveryShowRequest,
    LocalMutationEvidence, LocalMutationState, OperationId, SavedMessage, UuidIdentity,
    WaitUnavailable, WakeFailure, WakeFailureReason, WakeFailureStage, WakeMutationRequest,
    WakeMutationResult, WakeNextAction, WakeNotFound, WakeSendRequest, WakeShowRequest,
    WakeSnapshot, WakeState, WakeWaitOutcome, WakeWaitRequest, WakeWaitResult,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

/// Wake and delivery operations over automation storage.
pub struct WakeOperations<'service> {
    service_id: &'service UuidIdentity,
    store: Option<&'service Arc<Mutex<AutomationStore>>>,
    /// Concurrent first-fire waits share these permits; a handle without them cannot wait.
    wait_permits: Option<&'service Arc<Semaphore>>,
}

/// How often a first-fire wait checks for new automation events.
const WAKE_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long a first-fire wait runs when the caller names no timeout.
const DEFAULT_WAKE_WAIT_SECONDS: u32 = 60;
/// The longest a single first-fire wait may run.
const MAX_WAKE_WAIT_SECONDS: u32 = 1500;

impl<'service> WakeOperations<'service> {
    pub(crate) fn new(
        service_id: &'service UuidIdentity,
        store: Option<&'service Arc<Mutex<AutomationStore>>>,
    ) -> Self {
        Self {
            service_id,
            store,
            wait_permits: None,
        }
    }

    /// Lets this handle run first-fire waits, sharing `wait_permits` with every other wait.
    pub(crate) fn with_wait_capacity(mut self, wait_permits: &'service Arc<Semaphore>) -> Self {
        self.wait_permits = Some(wait_permits);
        self
    }

    fn store(
        &self,
        context: &WakeFailureContext,
    ) -> Result<&'service Arc<Mutex<AutomationStore>>, WakeFailure> {
        self.store.ok_or_else(|| {
            wake_failure(
                context.clone(),
                WakeFailureReason::AutomationUnavailable,
                LocalMutationState::None,
            )
        })
    }

    /// Creates a wake, or replays the wake an earlier request with this operation created.
    pub async fn wake_send(&self, request: WakeSendRequest) -> Result<WakeSnapshot, WakeFailure> {
        let context = WakeFailureContext::operation(request.operation_id.clone());
        let store = self.store(&context)?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let timing = serde_json::to_value(request.timing).and_then(serde_json::from_value);
        let expiry = serde_json::to_value(request.expiry).and_then(serde_json::from_value);
        let (Ok(timing), Ok(expiry)) = (timing, expiry) else {
            return Err(wake_failure(
                context,
                WakeFailureReason::InvalidField {
                    field: "timing".into(),
                    constraint: "Use a valid UTC instant or bounded duration.".into(),
                },
                LocalMutationState::None,
            ));
        };
        let created = store
            .lock()
            .await
            .create_wakeup(&WakeCreate {
                operation_id: request.operation_id,
                message: request.message,
                timing,
                expiry,
                now_ms,
            })
            .await;
        match created {
            // Creation and replay return the cursor time of the durable creation receipt.
            Ok(record) => {
                let observed = record.definition.created_at_ms;
                crate::wakeup_projection::snapshot(record, self.service_id, observed).map_err(
                    |()| {
                        wake_failure(
                            context,
                            WakeFailureReason::InvalidRecord,
                            LocalMutationState::Committed,
                        )
                    },
                )
            }
            Err(error) => {
                let mutation = if matches!(error, StorageError::Database(_)) {
                    LocalMutationState::Unknown
                } else {
                    LocalMutationState::None
                };
                Err(wake_failure(context, storage_reason(error), mutation))
            }
        }
    }

    /// Shows one wake as of now.
    pub async fn wake_show(&self, request: WakeShowRequest) -> Result<WakeSnapshot, WakeFailure> {
        let context = WakeFailureContext::wakeup(request.wakeup_id.clone());
        let store = self.store(&context)?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let read = store
            .lock()
            .await
            .read_wakeup::<SavedMessage>(&request.wakeup_id)
            .await;
        match read {
            Ok(record) => crate::wakeup_projection::snapshot(record, self.service_id, now_ms)
                .map_err(|()| {
                    wake_failure(
                        context,
                        WakeFailureReason::InvalidRecord,
                        LocalMutationState::None,
                    )
                }),
            Err(error) => Err(wake_failure(
                context,
                storage_reason(error),
                LocalMutationState::None,
            )),
        }
    }

    /// Lists wakes newest first; the page shrinks until its response fits `budget`.
    pub async fn wake_list(
        &self,
        request: AutomationPageRequest,
        budget: ResultByteBudget,
    ) -> Result<AutomationPage<WakeSnapshot>, WakeFailure> {
        let context = WakeFailureContext::default();
        let store = self.store(&context)?;
        let fail = |reason| wake_failure(context.clone(), reason, LocalMutationState::None);
        let digest = crate::automation_collection_cursor::filter_digest(&json!({}))
            .map_err(|()| fail(WakeFailureReason::InvalidRecord))?;
        let after = match request.cursor {
            None => None,
            Some(cursor) => {
                let decoded = crate::automation_collection_cursor::decode(
                    &cursor,
                    self.service_id,
                    "wake/list",
                    &digest,
                )
                .map_err(|_| {
                    fail(invalid_field(
                        "cursor",
                        "Cursor belongs to another service/collection or is malformed; start a fresh listing.",
                    ))
                })?;
                let (Ok(upper), Ok(last)) = (
                    decoded.upper_key.1.try_into(),
                    decoded.last_key.1.try_into(),
                ) else {
                    return Err(fail(invalid_field(
                        "cursor",
                        "Cursor wake identities must be UUIDv7.",
                    )));
                };
                Some(WakeListPosition {
                    upper_created_at_ms: decoded.upper_key.0,
                    upper_wakeup_id: upper,
                    created_at_ms: decoded.last_key.0,
                    wakeup_id: last,
                })
            }
        };
        let page = store
            .lock()
            .await
            .list_wakeups::<SavedMessage>(after, u32::from(request.limit))
            .await
            .map_err(|_| fail(WakeFailureReason::AutomationUnavailable))?;
        let now = chrono::Utc::now().timestamp_millis();
        self.fit_wake_page(page, &digest, now, budget)
            .map_err(|()| fail(WakeFailureReason::InvalidRecord))
    }

    fn fit_wake_page(
        &self,
        page: automation_storage::WakeListPage<SavedMessage>,
        digest: &str,
        now: i64,
        budget: ResultByteBudget,
    ) -> Result<AutomationPage<WakeSnapshot>, ()> {
        let positions: Vec<_> = page
            .records
            .iter()
            .map(|record| {
                (
                    record.definition.created_at_ms,
                    record.definition.wakeup_id.clone(),
                )
            })
            .collect();
        let upper = page
            .next
            .as_ref()
            .map(|position| {
                (
                    position.upper_created_at_ms,
                    position.upper_wakeup_id.clone(),
                )
            })
            .or_else(|| positions.last().cloned());
        let mut records = page
            .records
            .into_iter()
            .map(|record| crate::wakeup_projection::snapshot(record, self.service_id, now))
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = page.next;
        loop {
            let next_cursor = next
                .as_ref()
                .map(|position| {
                    crate::automation_collection_cursor::encode(
                        &crate::automation_collection_cursor::CollectionCursor {
                            version: 1,
                            service_id: self.service_id.clone(),
                            collection: "wake/list".into(),
                            upper_key: (
                                position.upper_created_at_ms,
                                position.upper_wakeup_id.as_str().into(),
                            ),
                            last_key: (position.created_at_ms, position.wakeup_id.as_str().into()),
                            filter_digest: digest.to_owned(),
                        },
                    )
                })
                .transpose()?;
            let candidate = AutomationPage {
                records: records.clone(),
                next_cursor,
            };
            if budget.admits(&candidate) {
                return Ok(candidate);
            }
            records.pop();
            let index = records.len().checked_sub(1).ok_or(())?;
            let (created_at_ms, wakeup_id) = positions.get(index).ok_or(())?;
            let (upper_created_at_ms, upper_wakeup_id) = upper.as_ref().ok_or(())?;
            next = Some(WakeListPosition {
                upper_created_at_ms: *upper_created_at_ms,
                upper_wakeup_id: upper_wakeup_id.clone(),
                created_at_ms: *created_at_ms,
                wakeup_id: wakeup_id.clone(),
            });
        }
    }

    /// Pauses a wake; deliveries already handed to a native thread are not recalled.
    pub async fn wake_pause(
        &self,
        request: WakeMutationRequest,
    ) -> Result<WakeMutationResult, WakeFailure> {
        self.mutate(request, WakeAction::Pause).await
    }

    /// Resumes a paused wake.
    pub async fn wake_resume(
        &self,
        request: WakeMutationRequest,
    ) -> Result<WakeMutationResult, WakeFailure> {
        self.mutate(request, WakeAction::Resume).await
    }

    /// Cancels a wake; a cancelled wake is never implicitly recreated.
    pub async fn wake_cancel(
        &self,
        request: WakeMutationRequest,
    ) -> Result<WakeMutationResult, WakeFailure> {
        self.mutate(request, WakeAction::Cancel).await
    }

    async fn mutate(
        &self,
        request: WakeMutationRequest,
        action: WakeAction,
    ) -> Result<WakeMutationResult, WakeFailure> {
        let context = WakeFailureContext {
            operation_id: Some(request.operation_id.clone()),
            wakeup_id: Some(request.wakeup_id.clone()),
        };
        let store = self.store(&context)?;
        let now = chrono::Utc::now().timestamp_millis();
        let mutated = store
            .lock()
            .await
            .mutate_wakeup::<SavedMessage>(&WakeMutation {
                operation_id: request.operation_id,
                wakeup_id: request.wakeup_id,
                action,
                now_ms: now,
            })
            .await;
        match mutated {
            Ok(result) => crate::wakeup_projection::project_mutation(result, self.service_id)
                .map_err(|()| {
                    wake_failure(
                        context,
                        WakeFailureReason::InvalidRecord,
                        LocalMutationState::Committed,
                    )
                }),
            Err(error) => {
                let mutation = if matches!(error, StorageError::Database(_)) {
                    LocalMutationState::Unknown
                } else {
                    LocalMutationState::None
                };
                let reason = match error {
                    StorageError::WakeNotFound => WakeFailureReason::ResourceNotFound,
                    StorageError::OperationConflict => WakeFailureReason::OperationConflict,
                    StorageError::WakeLifecycleConflict { .. } => {
                        WakeFailureReason::LifecycleConflict
                    }
                    StorageError::Database(_) => WakeFailureReason::AutomationUnavailable,
                    _ => WakeFailureReason::InvalidRecord,
                };
                Err(wake_failure(context, reason, mutation))
            }
        }
    }

    /// Shows one wake delivery and its retained native evidence.
    pub async fn delivery_show(
        &self,
        request: DeliveryShowRequest,
    ) -> Result<DeliveryInspection, WakeFailure> {
        let context = WakeFailureContext::default();
        let store = self.store(&context)?;
        let read = store.lock().await.read_delivery(&request.delivery_id).await;
        let Ok(record) = read else {
            let mut failure = wake_failure(
                context,
                WakeFailureReason::ResourceNotFound,
                LocalMutationState::None,
            );
            failure.message = format!(
                "Delivery {} was not found; for a push record, run agent-collaboration show <link>.",
                request.delivery_id.as_str()
            );
            return Err(failure);
        };
        crate::delivery_projection::snapshot(record).map_err(|()| {
            wake_failure(
                context,
                WakeFailureReason::InvalidRecord,
                LocalMutationState::None,
            )
        })
    }

    /// Waits up to `timeoutSeconds` for a wake-up's first fire, or for a change that rules one
    /// out. Starts from the current state, or resumes after `after`, and always returns the
    /// cursor a later wait resumes from, so no change is lost between calls.
    pub async fn wake_wait_until_first_fire(
        &self,
        request: WakeWaitRequest,
    ) -> Result<WakeWaitResult, WakeWaitFailure> {
        let wakeup_id = request.wakeup_id.clone();
        let timeout_seconds = request.timeout_seconds.unwrap_or(DEFAULT_WAKE_WAIT_SECONDS);
        if !(1..=MAX_WAKE_WAIT_SECONDS).contains(&timeout_seconds) {
            return Err(WakeWaitFailure::InvalidField {
                field: "timeoutSeconds",
                constraint: "Wait between 1 and 1500 seconds.",
            });
        }
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(u64::from(timeout_seconds));
        let (store, permit) = self.wait_admission(&wakeup_id)?;
        let mut observation = match request.after {
            None => {
                let (observation, subscription) = crate::wakeup_subscription::start(
                    Arc::clone(store),
                    self.service_id.clone(),
                    WakeShowRequest {
                        wakeup_id: wakeup_id.clone(),
                    },
                    permit,
                )
                .await
                .map_err(|error| match error {
                    StorageError::WakeNotFound => WakeWaitFailure::not_found(wakeup_id.clone()),
                    _ => WakeWaitFailure::unavailable(wakeup_id.clone()),
                })?;
                if let Some(outcome) = settled_outcome(&subscription.snapshot) {
                    return Ok(WakeWaitResult {
                        wakeup_id,
                        outcome,
                        cursor: subscription.after,
                    });
                }
                observation
            }
            Some(cursor) => WakeSubscriptionState::resume(
                Arc::clone(store),
                self.service_id.clone(),
                wakeup_id.clone(),
                &cursor,
                permit,
            )
            .await
            .map_err(|error| match error {
                WakeResumeError::InvalidCursor => WakeWaitFailure::InvalidField {
                    field: "after",
                    constraint: "Use a cursor an earlier wait on this service returned, or omit it.",
                },
                WakeResumeError::NotFound => WakeWaitFailure::not_found(wakeup_id.clone()),
                WakeResumeError::Unavailable => WakeWaitFailure::unavailable(wakeup_id.clone()),
            })?,
        };
        loop {
            let changes = observation
                .next_changes()
                .await
                .map_err(|_| WakeWaitFailure::unavailable(wakeup_id.clone()))?;
            if let Some(changed) = changes.into_iter().next() {
                return Ok(WakeWaitResult {
                    wakeup_id,
                    outcome: changed.change.into(),
                    cursor: changed.cursor,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                let cursor = observation
                    .cursor()
                    .map_err(|_| WakeWaitFailure::unavailable(wakeup_id.clone()))?;
                return Ok(WakeWaitResult {
                    wakeup_id,
                    outcome: WakeWaitOutcome::TimedOut,
                    cursor,
                });
            }
            tokio::time::sleep_until(
                (tokio::time::Instant::now() + WAKE_WAIT_POLL_INTERVAL).min(deadline),
            )
            .await;
        }
    }

    fn wait_admission(
        &self,
        wakeup_id: &WakeupId,
    ) -> Result<(&'service Arc<Mutex<AutomationStore>>, OwnedSemaphorePermit), WakeWaitFailure>
    {
        let permit = self
            .wait_permits
            .and_then(|permits| Arc::clone(permits).try_acquire_owned().ok());
        match (self.store, permit) {
            (Some(store), Some(permit)) => Ok((store, permit)),
            _ => Err(WakeWaitFailure::unavailable(wakeup_id.clone())),
        }
    }
}

/// A snapshot that already answers a first-fire wait: it fired, or can no longer fire.
fn settled_outcome(snapshot: &WakeSnapshot) -> Option<WakeWaitOutcome> {
    if let Some(fire) = snapshot.first_fire.clone() {
        return Some(WakeWaitOutcome::Fired { fire });
    }
    match snapshot.state {
        WakeState::Active => None,
        WakeState::Paused => Some(WakeWaitOutcome::Paused),
        WakeState::Cancelled => Some(WakeWaitOutcome::Cancelled),
        WakeState::Expired => Some(WakeWaitOutcome::Expired),
        WakeState::Finished => Some(WakeWaitOutcome::FinishedWithoutFiring),
    }
}

/// The identities a wake failure reports back, taken from the request that failed.
#[derive(Clone, Debug, Default)]
pub(crate) struct WakeFailureContext {
    pub(crate) operation_id: Option<OperationId>,
    pub(crate) wakeup_id: Option<WakeupId>,
}

impl WakeFailureContext {
    fn operation(operation_id: OperationId) -> Self {
        Self {
            operation_id: Some(operation_id),
            wakeup_id: None,
        }
    }

    fn wakeup(wakeup_id: WakeupId) -> Self {
        Self {
            operation_id: None,
            wakeup_id: Some(wakeup_id),
        }
    }
}

/// A wake failure with the stage, message and next action its reason implies.
pub(crate) fn wake_failure(
    context: WakeFailureContext,
    reason: WakeFailureReason,
    mutation: LocalMutationState,
) -> WakeFailure {
    let (stage,next_action,message)=match &reason {
        WakeFailureReason::InvalidField {constraint,..}=>(WakeFailureStage::Validation,WakeNextAction::CorrectRequest,constraint.clone()),
        WakeFailureReason::ResourceNotFound=>(WakeFailureStage::Inspection,WakeNextAction::VerifyResourceAddress,"Wake-up was not found in this service; verify the service and wakeupId.".into()),
        WakeFailureReason::OperationConflict=>(WakeFailureStage::Admission,WakeNextAction::InspectOperation,"Operation identity belongs to a different request; inspect it before submitting new work.".into()),
        WakeFailureReason::LifecycleConflict=>(WakeFailureStage::Validation,WakeNextAction::InspectWakeup,"This wake cannot perform that lifecycle transition. Inspect its state; cancelled or expired reminders are not implicitly recreated.".into()),
        WakeFailureReason::Overloaded=>(WakeFailureStage::Admission,WakeNextAction::RetryLater,"Request capacity exceeded; no wake mutation was dispatched.".into()),
        _ if matches!(mutation,LocalMutationState::Unknown|LocalMutationState::Committed)=>(WakeFailureStage::Storage,WakeNextAction::InspectOperation,"Local mutation may exist; inspect or replay the same operation identity. Do not create a new identity blindly.".into()),
        _=>(WakeFailureStage::Storage,WakeNextAction::RetryLater,"Automation storage is unavailable or inconsistent; no new native submission was dispatched.".into()),
    };
    WakeFailure {
        reason,
        stage,
        message,
        operation_id: context.operation_id,
        wakeup_id: context.wakeup_id,
        effects: LocalMutationEvidence::Local { mutation },
        next_action,
    }
}

pub(crate) fn invalid_field(field: &str, constraint: &str) -> WakeFailureReason {
    WakeFailureReason::InvalidField {
        field: field.into(),
        constraint: constraint.into(),
    }
}

fn storage_reason(error: StorageError) -> WakeFailureReason {
    match error {
        StorageError::WakeNotFound => WakeFailureReason::ResourceNotFound,
        StorageError::OperationConflict => WakeFailureReason::OperationConflict,
        StorageError::Database(_) => WakeFailureReason::AutomationUnavailable,
        StorageError::InvalidTiming(error) => WakeFailureReason::InvalidField {
            field: "timing".into(),
            constraint: error.to_string(),
        },
        StorageError::InvalidWake { field, reason } => WakeFailureReason::InvalidField {
            field: field.into(),
            constraint: reason.into(),
        },
        _ => WakeFailureReason::InvalidRecord,
    }
}

/// Why a first-fire wait could not start.
#[derive(Clone, Debug)]
pub enum WakeWaitFailure {
    /// No wait capacity or storage; reconnect the wait without recreating the reminder.
    Unavailable(WaitUnavailable),
    NotFound(WakeNotFound),
    /// The wait's cursor or timeout is invalid.
    InvalidField {
        field: &'static str,
        constraint: &'static str,
    },
}

impl WakeWaitFailure {
    pub(crate) fn unavailable(wakeup_id: WakeupId) -> Self {
        Self::Unavailable(WaitUnavailable {
            kind: collaboration_protocol::WaitUnavailableKind::WaitUnavailable,
            stage: collaboration_protocol::WaitStage::WaitForFirstFire,
            message: "Wake wait unavailable; reconnect the wait without recreating or cancelling the reminder.".into(),
            wakeup_id,
            first_occurrence_id: None,
            effects: collaboration_protocol::WaitUnavailableEffects {
                first_fire: collaboration_protocol::UnknownFire::Unknown,
                wakeup_mutation: collaboration_protocol::NoMutation::None,
            },
            next_action: collaboration_protocol::WaitNextAction::ReconnectWait,
        })
    }

    pub(crate) fn not_found(wakeup_id: WakeupId) -> Self {
        Self::NotFound(WakeNotFound {
            kind: collaboration_protocol::WakeNotFoundKind::WakeNotFound,
            stage: collaboration_protocol::WaitStage::WaitForFirstFire,
            message: "Wake-up was not found in this service; verify the wake identity. Historical firing is unknown.".into(),
            wakeup_id,
            first_occurrence_id: None,
            effects: collaboration_protocol::UnknownFirstFire {
                first_fire: collaboration_protocol::UnknownFire::Unknown,
            },
            next_action: collaboration_protocol::VerifyWakeupAddress::VerifyWakeupAddress,
        })
    }
}

impl CollaborationRejection for WakeFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.reason {
            WakeFailureReason::Overloaded => Some(CollaborationRejectionReason::Overloaded),
            WakeFailureReason::InvalidField { .. } => {
                Some(CollaborationRejectionReason::InvalidShape)
            }
            WakeFailureReason::OperationConflict => {
                Some(CollaborationRejectionReason::ConflictingRequest)
            }
            WakeFailureReason::AutomationUnavailable
            | WakeFailureReason::ResourceNotFound
            | WakeFailureReason::InvalidRecord
            | WakeFailureReason::LifecycleConflict => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            "Wake operation failed",
            self,
        )
    }
}

impl CollaborationRejection for WakeWaitFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self {
            Self::InvalidField { .. } => Some(CollaborationRejectionReason::InvalidShape),
            Self::Unavailable(_) | Self::NotFound(_) => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        match self {
            Self::Unavailable(data) => PublishedRejection::typed(
                PublishedRejection::OPERATION_FAILED,
                "Wake wait unavailable",
                data,
            ),
            Self::NotFound(data) => PublishedRejection::typed(
                PublishedRejection::OPERATION_FAILED,
                "Wake not found",
                data,
            ),
            Self::InvalidField { field, constraint } => PublishedRejection::typed(
                PublishedRejection::INVALID_PARAMS,
                *constraint,
                &json!({
                    "kind":"invalidField",
                    "stage":"waitForFirstFire",
                    "field":field,
                    "constraint":constraint,
                    "message":constraint,
                }),
            ),
        }
    }
}
