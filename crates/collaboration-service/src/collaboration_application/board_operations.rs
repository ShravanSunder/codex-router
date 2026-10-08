//! Board operations: projects, boards, topics, threads, posts, inbox and thread subscriptions.
//!
//! Every board write enters the one serialized board store. Writes that can open or close a
//! subscription window then reconcile the reader's delivery owner, using the delivery owner's
//! clock for durable window deadlines.
use super::{
    CollaborationRejection, CollaborationRejectionReason, PublishedRejection, ResultByteBudget,
};
use crate::ServiceIdentity;
use collaboration_protocol::{
    SubscriptionWaitBatch, ThreadSubscribeRequest, ThreadSubscriptionPresence,
    ThreadSubscriptionState, ThreadSubscriptionView,
    ThreadSubscriptionWaitFilter as WaitRequestFilter, ThreadSubscriptionWaitRequest,
    ThreadSubscriptionWaitResult, ThreadSubscriptionsRequest, ThreadSubscriptionsResult,
    ThreadUnsubscribeRequest,
};
use message_board::*;
use message_board_storage::BoardStore;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

const TARGET_PRESENCE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Board operations over the Router's board store and subscription delivery owner.
pub struct BoardOperations<'service> {
    identity: &'service ServiceIdentity,
}

/// A read or write that needs only the board store.
macro_rules! store_operations {
    ($($(#[$documentation:meta])* $operation:ident($request:ty) -> $result:ty => $store_method:ident;)*) => {
        $(
            $(#[$documentation])*
            pub async fn $operation(&self, request: $request) -> Result<$result, BoardError> {
                self.store()?.lock().await.$store_method(request).await
            }
        )*
    };
}

/// A write that may change the actor's subscription windows, so delivery reconciles after it.
macro_rules! reconciled_writes {
    ($($(#[$documentation:meta])* $operation:ident($request:ty) -> $result:ty => $store_method:ident;)*) => {
        $(
            $(#[$documentation])*
            pub async fn $operation(&self, request: $request) -> Result<$result, BoardError> {
                let store = self.store()?;
                let reader = request.actor.clone();
                // The delivery owner's clock stamps durable subscription window deadlines.
                let result = store
                    .lock()
                    .await
                    .$store_method(request, self.identity.subscription_clock.now())
                    .await?;
                self.reconcile_after_write(reader).await?;
                Ok(result)
            }
        )*
    };
}

impl<'service> BoardOperations<'service> {
    pub(crate) fn new(identity: &'service ServiceIdentity) -> Self {
        Self { identity }
    }

    fn store(&self) -> Result<&'service Arc<Mutex<BoardStore>>, BoardError> {
        self.identity
            .board
            .as_ref()
            .ok_or_else(board_storage_unavailable)
    }

    async fn reconcile_after_write(&self, reader: Identity) -> Result<(), BoardError> {
        if let Some(service) = self.identity.subscription_delivery.as_ref()
            && let Err(error) = service.reconcile_reader(reader).await
        {
            tracing::warn!(error=%error,"subscription owner reconciliation failed after a board write");
            return Err(subscription_reconciliation_failed());
        }
        Ok(())
    }

    /// Every request naming a local repository must name one of this Router's.
    fn require_local_repository(&self, repository: &RepositoryRef) -> Result<(), BoardError> {
        if let RepositoryRef::Local { service_id, .. } = repository
            && service_id.as_str() != String::from(self.identity.service_id.clone())
        {
            return Err(BoardError::invalid_field(
                "repository.serviceId",
                "must match the selected board service",
            ));
        }
        Ok(())
    }

    store_operations! {
        discovery_search(DiscoverySearchRequest) -> DiscoverySearchResult => search_discovery;
        message_search(MessageSearchRequest) -> MessageSearchResult => search_messages;
        project_create(ProjectCreateRequest) -> ProjectCreateResult => create_project;
        project_update(ProjectUpdateRequest) -> ProjectUpdateResult => update_project;
        project_show(ProjectShowRequest) -> ProjectShowResult => show_project;
        repository_list(RepositoryListRequest) -> RepositoryListResult => list_repositories;
        board_create(BoardCreateRequest) -> BoardCreateResult => create_board;
        board_update(BoardUpdateRequest) -> BoardUpdateResult => update_board;
        board_show(BoardShowRequest) -> BoardShowResult => show_board;
        board_list(BoardListRequest) -> BoardListResult => list_boards;
        board_archive(BoardArchiveRequest) -> BoardArchiveResult => archive_board;
        topic_create(TopicCreateRequest) -> TopicCreateResult => create_topic;
        topic_update(TopicUpdateRequest) -> TopicUpdateResult => update_topic;
        topic_list(TopicListRequest) -> TopicListResult => list_topics;
        message_show(MessageShowRequest) -> MessageShowResult => show_message;
        message_list(MessageListRequest) -> MessageListResult => list_messages;
        thread_show(ThreadShowRequest) -> ThreadShowResult => show_thread;
        thread_unresolve(ThreadUnresolveRequest) -> ThreadUnresolveResult => unresolve_thread;
        thread_watch(ThreadWatchRequest) -> ThreadWatchResult => watch_thread;
        topic_watch(TopicWatchRequest) -> TopicWatchResult => watch_topic;
        thread_list(ThreadListRequest) -> ThreadListResult => list_threads;
        thread_participant_list(ThreadParticipantListRequest) -> ThreadParticipantListResult => list_thread_participants;
        inbox_fetch(InboxFetchRequest) -> InboxFetchResult => fetch_inbox;
        inbox_acknowledge(InboxAcknowledgeRequest) -> InboxAcknowledgeResult => acknowledge_inbox;
        inbox_projects(InboxProjectsRequest) -> InboxProjectsResult => list_inbox_projects;
    }

    reconciled_writes! {
        message_post(MessagePostRequest) -> MessagePostResult => post_message;
        thread_resolve(ThreadResolveRequest) -> ThreadResolveResult => resolve_thread;
        thread_unwatch(ThreadUnwatchRequest) -> ThreadUnwatchResult => unwatch_thread;
        topic_unwatch(TopicWatchRequest) -> TopicWatchResult => unwatch_topic;
        thread_create(ThreadCreateRequest) -> ThreadCreateResult => create_thread;
        thread_join(ThreadJoinRequest) -> ThreadJoinResult => join_thread;
        thread_leave(ThreadLeaveRequest) -> ThreadLeaveResult => leave_thread;
    }

    /// Lists projects, optionally those attached to one repository of this Router's machine.
    pub async fn project_list(
        &self,
        request: ProjectListRequest,
    ) -> Result<ProjectListResult, BoardError> {
        let store = self.store()?;
        if let Some(repository) = &request.repository {
            self.require_local_repository(repository)?;
        }
        store.lock().await.list_projects(request).await
    }

    /// Attaches a repository of this Router's machine to a project.
    pub async fn repository_attach(
        &self,
        request: RepositoryAttachRequest,
    ) -> Result<RepositoryAttachResult, BoardError> {
        let store = self.store()?;
        self.require_local_repository(&request.repository)?;
        store.lock().await.attach_repository(request).await
    }

    /// Detaches a repository of this Router's machine from a project.
    pub async fn repository_detach(
        &self,
        request: RepositoryDetachRequest,
    ) -> Result<RepositoryDetachResult, BoardError> {
        let store = self.store()?;
        self.require_local_repository(&request.repository)?;
        store.lock().await.detach_repository(request).await
    }

    /// Subscribes the actor to a thread or topic and reports the actor's delivery presence.
    pub async fn thread_subscribe(
        &self,
        request: ThreadSubscribeRequest,
    ) -> Result<ThreadSubscriptionView, BoardError> {
        let subscriptions = self.subscription_dependencies()?;
        let reader = request.actor.clone();
        let record = subscriptions
            .store
            .lock()
            .await
            .subscribe_thread_subscription(
                ThreadSubscriptionSubscribeRequest {
                    reader: request.actor,
                    scope: request.scope,
                    policy: request.policy,
                },
                self.identity.subscription_clock.now(),
            )
            .await?;
        if let Err(error) = subscriptions
            .delivery
            .reconcile_reader(reader.clone())
            .await
        {
            tracing::warn!(error=%error,"subscription owner reconciliation failed after subscribe");
            return Err(subscription_owner_reconciliation_failed());
        }
        let presence = presence_for_reader(&reader, subscriptions.presence).await;
        Ok(subscription_view(record, presence))
    }

    /// Ends the actor's subscription to a thread or topic.
    pub async fn thread_unsubscribe(
        &self,
        request: ThreadUnsubscribeRequest,
    ) -> Result<ThreadSubscriptionView, BoardError> {
        let subscriptions = self.subscription_dependencies()?;
        let reader = request.actor.clone();
        let record = subscriptions
            .store
            .lock()
            .await
            .unsubscribe_thread_subscription(
                ThreadSubscriptionUnsubscribeRequest {
                    reader: request.actor,
                    scope: request.scope,
                },
                self.identity.subscription_clock.now(),
            )
            .await?;
        if let Err(error) = subscriptions
            .delivery
            .reconcile_reader(reader.clone())
            .await
        {
            tracing::warn!(error=%error,"subscription owner reconciliation failed after unsubscribe");
            return Err(subscription_owner_reconciliation_failed());
        }
        let presence = presence_for_reader(&reader, subscriptions.presence).await;
        Ok(subscription_view(record, presence))
    }

    /// Lists the actor's subscriptions with its current delivery presence.
    pub async fn thread_subscriptions(
        &self,
        request: ThreadSubscriptionsRequest,
    ) -> Result<ThreadSubscriptionsResult, BoardError> {
        let subscriptions = self.subscription_dependencies()?;
        let records = subscriptions
            .store
            .lock()
            .await
            .list_reader_subscriptions(&request.actor, self.identity.subscription_clock.now())
            .await?;
        if let Err(error) = subscriptions
            .delivery
            .reconcile_reader(request.actor.clone())
            .await
        {
            tracing::warn!(error=%error,"subscription owner reconciliation failed after listing subscriptions");
        }
        let presence = presence_for_reader(&request.actor, subscriptions.presence).await;
        let subscriptions = records
            .into_iter()
            .map(|record| subscription_view(record, presence.clone()))
            .collect();
        Ok(ThreadSubscriptionsResult { subscriptions })
    }

    /// Waits up to `maxWaitSeconds` for the actor's next poll-mode batch.
    ///
    /// The batch settles when it is handed to this call's caller (G16). A notice's roots fit
    /// `maximum_root_notice_bytes`, which the caller's transport derives from its response limit.
    pub async fn thread_wait(
        &self,
        request: ThreadSubscriptionWaitRequest,
        maximum_root_notice_bytes: usize,
    ) -> Result<ThreadSubscriptionWaitResult, BoardError> {
        let subscriptions = self.subscription_dependencies()?;
        let filter = match request.filter {
            WaitRequestFilter::All {} => crate::SubscriptionWaitFilter::All,
            WaitRequestFilter::Roots { root_message_ids } => {
                crate::SubscriptionWaitFilter::Roots(root_message_ids)
            }
            WaitRequestFilter::Topic { topic_id } => crate::SubscriptionWaitFilter::Topic(topic_id),
        };
        let result = subscriptions
            .delivery
            .wait(
                request.actor,
                filter,
                request.max_wait_seconds,
                maximum_root_notice_bytes,
            )
            .await?;
        Ok(subscription_wait_result(result))
    }

    fn subscription_dependencies(&self) -> Result<SubscriptionDependencies<'service>, BoardError> {
        let store = self.store()?;
        let delivery = self
            .identity
            .subscription_delivery
            .as_ref()
            .ok_or_else(BoardError::board_unavailable)?;
        let presence = self
            .identity
            .subscription_presence
            .as_ref()
            .ok_or_else(BoardError::board_unavailable)?;
        Ok(SubscriptionDependencies {
            store,
            delivery,
            presence,
        })
    }
}

struct SubscriptionDependencies<'service> {
    store: &'service Arc<Mutex<BoardStore>>,
    delivery: &'service crate::SubscriptionDeliveryService,
    presence: &'service Arc<dyn crate::TargetPresenceProbe>,
}

impl CollaborationRejection for BoardError {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self.kind {
            BoardFailureKind::Overloaded => Some(CollaborationRejectionReason::Overloaded),
            BoardFailureKind::ArchivedBoard => Some(CollaborationRejectionReason::Archived),
            BoardFailureKind::InvalidField | BoardFailureKind::InvalidIdentity => {
                Some(CollaborationRejectionReason::InvalidShape)
            }
            BoardFailureKind::InvalidCursor => Some(CollaborationRejectionReason::CursorInvalid),
            BoardFailureKind::TopLevelMessageCooldown
            | BoardFailureKind::ReferenceTargetNotFound
            | BoardFailureKind::InvalidTopicName
            | BoardFailureKind::InvalidRootMessage
            | BoardFailureKind::PositionBeyondLatest
            | BoardFailureKind::InvalidAcknowledgement
            | BoardFailureKind::NameConflict
            | BoardFailureKind::ResourceNotFound
            | BoardFailureKind::ResourceAlreadyExists
            | BoardFailureKind::OutcomeUnknown
            | BoardFailureKind::BoardUnavailable
            | BoardFailureKind::ThreadResolved
            | BoardFailureKind::InvalidRecord
            | BoardFailureKind::ParticipantRequired
            | BoardFailureKind::OrchestratorRequired
            | BoardFailureKind::OrchestratorAlreadyExists
            | BoardFailureKind::ImplementerAlreadyExists
            | BoardFailureKind::OrchestratorHandoverRequired
            | BoardFailureKind::HandoverTargetNotParticipant
            | BoardFailureKind::StaleOrchestrator
            | BoardFailureKind::StaleImplementer
            | BoardFailureKind::SelfReplace
            | BoardFailureKind::OrchestratorRoleChange
            | BoardFailureKind::SessionTopicPost => None,
        }
    }

    fn published_rejection(&self) -> PublishedRejection {
        PublishedRejection::typed(
            PublishedRejection::OPERATION_FAILED,
            self.message.clone(),
            self,
        )
    }
}

/// Why a response bound leaves no room for a thread-wait notice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ThreadWaitBudgetError {
    #[error("the response bound leaves no room for a subscription wait response")]
    NoRoomForResponse,
    #[error("the response bound leaves no room for one subscription root")]
    NoRoomForOneRoot,
}

/// The bytes a wait notice's roots may take so the largest notice still fits `budget`.
///
/// The notice reserves room for the longest push line and held-since time; only its roots
/// shrink to fit.
pub fn thread_wait_root_notice_limit(
    budget: ResultByteBudget,
) -> Result<usize, ThreadWaitBudgetError> {
    const MAX_HELD_SINCE_WIRE_BYTES: usize = 64;
    let empty_roots = serde_json::json!([]);
    let largest_notice = serde_json::json!({
        "batch":{
            "kind":"notice",
            "pushId":"01890f2e-7b4c-7cc0-98c4-000000000002",
            "line":"\\".repeat(collaboration_protocol::MAX_PUSH_LINE_BYTES),
            "held":true,
            "heldSince":"x".repeat(MAX_HELD_SINCE_WIRE_BYTES),
            "draining":true,
            "roots":empty_roots,
        }
    });
    let empty_roots_bytes = serde_json::to_vec(&empty_roots)
        .map_err(|_| ThreadWaitBudgetError::NoRoomForResponse)?
        .len();
    let fixed_response_bytes = budget
        .response_bytes(&largest_notice)
        .and_then(|bytes| bytes.checked_sub(empty_roots_bytes))
        .ok_or(ThreadWaitBudgetError::NoRoomForResponse)?;
    let maximum_root_notice_bytes = budget
        .response_limit_bytes()
        .checked_sub(fixed_response_bytes)
        .ok_or(ThreadWaitBudgetError::NoRoomForResponse)?;
    let largest_root = serde_json::json!([{
        "rootId":"01890f2e-7b4c-7cc0-98c4-000000000002",
        "topicId":"01890f2e-7b4c-7cc0-98c4-000000000003",
        "fromSequence":i64::MAX,
        "throughSequence":i64::MAX,
        "messageCount":u64::MAX,
    }]);
    let minimum_root_notice_bytes = serde_json::to_vec(&largest_root)
        .map_err(|_| ThreadWaitBudgetError::NoRoomForOneRoot)?
        .len();
    if maximum_root_notice_bytes < minimum_root_notice_bytes {
        return Err(ThreadWaitBudgetError::NoRoomForOneRoot);
    }
    Ok(maximum_root_notice_bytes)
}

/// The board store is not open on this Router.
fn board_storage_unavailable() -> BoardError {
    BoardError {
        kind: BoardFailureKind::BoardUnavailable,
        stage: BoardFailureStage::Admission,
        message: "Board storage unavailable; inspect the selected debug/service profile.".into(),
        next_action: BoardNextAction::RetryLater,
        details: BoardErrorDetails::None,
    }
}

fn subscription_reconciliation_failed() -> BoardError {
    BoardError {
        kind: BoardFailureKind::OutcomeUnknown,
        stage: BoardFailureStage::Storage,
        message: "The board change was saved, but subscription delivery could not synchronize. Inspect the current subscription state before retrying.".to_owned(),
        next_action: BoardNextAction::RetryLater,
        details: BoardErrorDetails::None,
    }
}

fn subscription_owner_reconciliation_failed() -> BoardError {
    BoardError {
        kind: BoardFailureKind::OutcomeUnknown,
        stage: BoardFailureStage::Storage,
        message: "The subscription change was saved, but its delivery owner could not synchronize. Inspect the current subscription state before retrying.".to_owned(),
        next_action: BoardNextAction::RetryLater,
        details: BoardErrorDetails::None,
    }
}

fn subscription_wait_result(
    result: Option<crate::SubscriptionWaitResult>,
) -> ThreadSubscriptionWaitResult {
    let batch = result.map(|result| match result {
        crate::SubscriptionWaitResult::Notice {
            push_id,
            line,
            batch,
        } => SubscriptionWaitBatch::Notice {
            push_id,
            line,
            held: batch.held,
            held_since: batch.held_since,
            draining: batch.draining,
            roots: batch.roots,
        },
        crate::SubscriptionWaitResult::Ranges { batch } => SubscriptionWaitBatch::Ranges {
            held: batch.held,
            held_since: batch.held_since,
            draining: batch.draining,
            roots: batch.roots,
        },
    });
    ThreadSubscriptionWaitResult { batch }
}

fn subscription_view(
    record: ThreadSubscriptionRecord,
    presence: ThreadSubscriptionPresence,
) -> ThreadSubscriptionView {
    let state = match record.state() {
        SubscriptionState::Active => ThreadSubscriptionState::Active,
        SubscriptionState::Draining => ThreadSubscriptionState::Draining,
        SubscriptionState::Ended { .. } => ThreadSubscriptionState::Ended,
    };
    let pending_count = record.roots().iter().map(|root| root.pending_count()).sum();
    let held_since = record
        .roots()
        .iter()
        .filter_map(|root| root.held_since())
        .min();
    let next_retry_at = record
        .roots()
        .iter()
        .filter_map(|root| root.next_retry_at())
        .min();
    ThreadSubscriptionView {
        scope: record.scope().clone(),
        policy: record.policy().clone(),
        state,
        end_reason: record.state().end_reason(),
        expires_at: record.expires_at(),
        pending_count,
        presence,
        held_since,
        next_retry_at,
        last_outcome: record.last_outcome().cloned(),
    }
}

async fn presence_for_reader(
    reader: &Identity,
    presence_probe: &Arc<dyn crate::TargetPresenceProbe>,
) -> ThreadSubscriptionPresence {
    let Identity::Session { session } = reader else {
        return ThreadSubscriptionPresence::Unreachable {
            reason: "no session target".to_owned(),
        };
    };
    let target = match serde_json::to_value(session)
        .and_then(serde_json::from_value::<collaboration_protocol::SessionRef>)
    {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(error=%error,"subscription target identity could not be projected for presence");
            return ThreadSubscriptionPresence::Unreachable {
                reason: "presence unavailable".to_owned(),
            };
        }
    };
    match tokio::time::timeout(
        TARGET_PRESENCE_PROBE_TIMEOUT,
        presence_probe.presence(&target),
    )
    .await
    {
        Ok(Ok(crate::TargetPresence::Running)) => ThreadSubscriptionPresence::Running {},
        Ok(Ok(crate::TargetPresence::Wakeable)) => ThreadSubscriptionPresence::Wakeable {},
        Ok(Ok(crate::TargetPresence::Unreachable { reason })) => {
            ThreadSubscriptionPresence::Unreachable { reason }
        }
        Ok(Err(error)) => {
            tracing::warn!(error=%error,"subscription target presence probe failed");
            ThreadSubscriptionPresence::Unreachable {
                reason: "presence unavailable".to_owned(),
            }
        }
        Err(_elapsed) => {
            tracing::warn!(
                timeout_seconds = TARGET_PRESENCE_PROBE_TIMEOUT.as_secs(),
                "subscription target presence probe timed out"
            );
            ThreadSubscriptionPresence::Unreachable {
                reason: "presence unavailable".to_owned(),
            }
        }
    }
}
