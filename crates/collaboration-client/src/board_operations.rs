//! Typed board calls over the collaboration API; uncertain writes are never replayed.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
    ThreadSubscribeRequest, ThreadSubscriptionView, ThreadSubscriptionWaitFilter,
    ThreadSubscriptionWaitRequest, ThreadSubscriptionWaitResult, ThreadSubscriptionsRequest,
    ThreadSubscriptionsResult, ThreadUnsubscribeRequest,
};
use message_board::*;
use std::time::Duration;
#[derive(Debug, thiserror::Error)]
pub enum BoardClientError {
    #[error("{0}")]
    Rejected(Box<BoardError>),
    #[error("{message}")]
    OutcomeUnknown {
        resource: ResourceIdentity,
        message: &'static str,
        next_action: BoardNextAction,
    },
    #[error("wait result was lost after a possible handoff")]
    WaitOutcomeUnknown {
        actor: Identity,
        filter: ThreadSubscriptionWaitFilter,
    },
    #[error(transparent)]
    Connection(#[from] ClientError),
}

impl CollaborationClient {
    pub async fn board_discovery_search(
        &self,
        request: DiscoverySearchRequest,
    ) -> Result<DiscoverySearchResult, BoardClientError> {
        self.board_call("board_discovery_search", request).await
    }
    pub async fn board_message_search(
        &self,
        request: MessageSearchRequest,
    ) -> Result<MessageSearchResult, BoardClientError> {
        self.board_call("board_message_search", request).await
    }

    pub async fn board_project_create(
        &self,
        request: ProjectCreateRequest,
    ) -> Result<ProjectCreateResult, BoardClientError> {
        let resource = ResourceIdentity::Project {
            project_id: request.project_id.clone(),
        };
        self.board_mutation_call("board_project_create", request, resource)
            .await
    }
    pub async fn board_project_update(
        &self,
        request: ProjectUpdateRequest,
    ) -> Result<ProjectUpdateResult, BoardClientError> {
        let resource = ResourceIdentity::Project {
            project_id: request.project_id.clone(),
        };
        self.board_mutation_call("board_project_update", request, resource)
            .await
    }
    pub async fn board_project_show(
        &self,
        request: ProjectShowRequest,
    ) -> Result<ProjectShowResult, BoardClientError> {
        self.board_call("board_project_show", request).await
    }
    pub async fn board_project_list(
        &self,
        request: ProjectListRequest,
    ) -> Result<ProjectListResult, BoardClientError> {
        self.board_call("board_project_list", request).await
    }
    pub async fn board_repository_attach(
        &self,
        request: RepositoryAttachRequest,
    ) -> Result<RepositoryAttachResult, BoardClientError> {
        let resource = ResourceIdentity::Project {
            project_id: request.project_id.clone(),
        };
        self.board_mutation_call("board_repository_attach", request, resource)
            .await
    }
    pub async fn board_repository_detach(
        &self,
        request: RepositoryDetachRequest,
    ) -> Result<RepositoryDetachResult, BoardClientError> {
        let resource = ResourceIdentity::Project {
            project_id: request.project_id.clone(),
        };
        self.board_mutation_call("board_repository_detach", request, resource)
            .await
    }
    pub async fn board_repository_list(
        &self,
        request: RepositoryListRequest,
    ) -> Result<RepositoryListResult, BoardClientError> {
        self.board_call("board_repository_list", request).await
    }
    pub async fn board_create(
        &self,
        request: BoardCreateRequest,
    ) -> Result<BoardCreateResult, BoardClientError> {
        let resource = ResourceIdentity::Board {
            board_id: request.board_id.clone(),
        };
        self.board_mutation_call("board_create", request, resource)
            .await
    }
    pub async fn board_update(
        &self,
        request: BoardUpdateRequest,
    ) -> Result<BoardUpdateResult, BoardClientError> {
        let resource = ResourceIdentity::Board {
            board_id: request.board_id.clone(),
        };
        self.board_mutation_call("board_update", request, resource)
            .await
    }
    pub async fn board_show(
        &self,
        request: BoardShowRequest,
    ) -> Result<BoardShowResult, BoardClientError> {
        self.board_call("board_show", request).await
    }
    pub async fn board_list(
        &self,
        request: BoardListRequest,
    ) -> Result<BoardListResult, BoardClientError> {
        self.board_call("board_list", request).await
    }
    pub async fn board_archive(
        &self,
        request: BoardArchiveRequest,
    ) -> Result<BoardArchiveResult, BoardClientError> {
        let resource = ResourceIdentity::Board {
            board_id: request.board_id.clone(),
        };
        self.board_mutation_call("board_archive", request, resource)
            .await
    }
    pub async fn board_topic_create(
        &self,
        request: TopicCreateRequest,
    ) -> Result<TopicCreateResult, BoardClientError> {
        let resource = ResourceIdentity::Topic {
            topic_id: request.topic_id.clone(),
        };
        self.board_mutation_call("board_topic_create", request, resource)
            .await
    }
    pub async fn board_topic_update(
        &self,
        request: TopicUpdateRequest,
    ) -> Result<TopicUpdateResult, BoardClientError> {
        let resource = ResourceIdentity::Topic {
            topic_id: request.topic_id.clone(),
        };
        self.board_mutation_call("board_topic_update", request, resource)
            .await
    }
    pub async fn board_topic_list(
        &self,
        request: TopicListRequest,
    ) -> Result<TopicListResult, BoardClientError> {
        self.board_call("board_topic_list", request).await
    }
    pub async fn board_message_post(
        &self,
        request: MessagePostRequest,
    ) -> Result<MessagePostResult, BoardClientError> {
        let resource = ResourceIdentity::Message {
            message_id: request.message_id.clone(),
        };
        self.board_mutation_call("board_message_post", request, resource)
            .await
    }
    pub async fn board_message_show(
        &self,
        request: MessageShowRequest,
    ) -> Result<MessageShowResult, BoardClientError> {
        self.board_call("board_message_show", request).await
    }
    pub async fn board_message_list(
        &self,
        request: MessageListRequest,
    ) -> Result<MessageListResult, BoardClientError> {
        self.board_call("board_message_list", request).await
    }
    pub async fn board_thread_show(
        &self,
        request: ThreadShowRequest,
    ) -> Result<ThreadShowResult, BoardClientError> {
        self.board_call("board_thread_show", request).await
    }
    pub async fn board_thread_resolve(
        &self,
        request: ThreadResolveRequest,
    ) -> Result<ThreadResolveResult, BoardClientError> {
        let resource = ResourceIdentity::Thread {
            root_message_id: request.root_message_id.clone(),
        };
        self.board_mutation_call("board_thread_resolve", request, resource)
            .await
    }
    pub async fn board_thread_unresolve(
        &self,
        request: ThreadUnresolveRequest,
    ) -> Result<ThreadUnresolveResult, BoardClientError> {
        let resource = ResourceIdentity::Thread {
            root_message_id: request.root_message_id.clone(),
        };
        self.board_mutation_call("board_thread_unresolve", request, resource)
            .await
    }
    pub async fn board_thread_watch(
        &self,
        request: ThreadWatchRequest,
    ) -> Result<ThreadWatchResult, BoardClientError> {
        let resource = ResourceIdentity::Thread {
            root_message_id: request.root_message_id.clone(),
        };
        self.board_mutation_call("board_thread_watch", request, resource)
            .await
    }
    pub async fn board_thread_unwatch(
        &self,
        request: ThreadUnwatchRequest,
    ) -> Result<ThreadUnwatchResult, BoardClientError> {
        let resource = ResourceIdentity::Thread {
            root_message_id: request.root_message_id.clone(),
        };
        self.board_mutation_call("board_thread_unwatch", request, resource)
            .await
    }
    pub async fn board_topic_watch(
        &self,
        request: TopicWatchRequest,
    ) -> Result<TopicWatchResult, BoardClientError> {
        let resource = ResourceIdentity::Topic {
            topic_id: request.topic_id.clone(),
        };
        self.board_mutation_call("board_topic_watch", request, resource)
            .await
    }
    pub async fn board_topic_unwatch(
        &self,
        request: TopicWatchRequest,
    ) -> Result<TopicWatchResult, BoardClientError> {
        let resource = ResourceIdentity::Topic {
            topic_id: request.topic_id.clone(),
        };
        self.board_mutation_call("board_topic_unwatch", request, resource)
            .await
    }
    pub async fn board_thread_list(
        &self,
        request: ThreadListRequest,
    ) -> Result<ThreadListResult, BoardClientError> {
        self.board_call("board_thread_list", request).await
    }
    pub async fn board_thread_create(
        &self,
        request: ThreadCreateRequest,
    ) -> Result<ThreadCreateResult, BoardClientError> {
        let resource = ResourceIdentity::Message {
            message_id: request.message_id.clone(),
        };
        self.board_mutation_call("board_thread_create", request, resource)
            .await
    }
    pub async fn board_thread_join(
        &self,
        request: ThreadJoinRequest,
    ) -> Result<ThreadJoinResult, BoardClientError> {
        let resource = ResourceIdentity::Thread {
            root_message_id: request.root_message_id.clone(),
        };
        self.board_mutation_call("board_thread_join", request, resource)
            .await
    }
    pub async fn board_thread_leave(
        &self,
        request: ThreadLeaveRequest,
    ) -> Result<ThreadLeaveResult, BoardClientError> {
        let resource = ResourceIdentity::Thread {
            root_message_id: request.root_message_id.clone(),
        };
        self.board_mutation_call("board_thread_leave", request, resource)
            .await
    }
    pub async fn board_thread_participant_list(
        &self,
        request: ThreadParticipantListRequest,
    ) -> Result<ThreadParticipantListResult, BoardClientError> {
        self.board_call("board_thread_participant_list", request)
            .await
    }
    pub async fn board_thread_subscribe(
        &self,
        request: ThreadSubscribeRequest,
    ) -> Result<ThreadSubscriptionView, BoardClientError> {
        let resource = subscription_resource(&request.scope);
        self.board_mutation_call("board_thread_subscribe", request, resource)
            .await
    }
    pub async fn board_thread_unsubscribe(
        &self,
        request: ThreadUnsubscribeRequest,
    ) -> Result<ThreadSubscriptionView, BoardClientError> {
        let resource = subscription_resource(&request.scope);
        self.board_mutation_call("board_thread_unsubscribe", request, resource)
            .await
    }
    pub async fn board_thread_subscriptions(
        &self,
        request: ThreadSubscriptionsRequest,
    ) -> Result<ThreadSubscriptionsResult, BoardClientError> {
        self.board_call("board_thread_subscriptions", request).await
    }
    pub async fn board_thread_wait(
        &self,
        request: ThreadSubscriptionWaitRequest,
        timeout: Duration,
    ) -> Result<ThreadSubscriptionWaitResult, BoardClientError> {
        request
            .validate()
            .map_err(|_| ClientError::InvalidRequest("invalid Thread Wait request"))?;
        let actor = request.actor.clone();
        let filter = request.filter.clone();
        let params = serde_json::to_value(&request)
            .map_err(|_| ClientError::Protocol("invalid Thread Wait request"))?;
        let result = match self
            .connection
            .call_with_timeout("board_thread_wait", params, timeout)
            .await
        {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                return Err(BoardClientError::Rejected(Box::new(
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid board failure"))?,
                )));
            }
            Err(ClientError::Timeout)
            | Err(ClientError::Transport(_))
            | Err(ClientError::Protocol(_)) => {
                return Err(BoardClientError::WaitOutcomeUnknown { actor, filter });
            }
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_value(result) {
            Ok(result) => Ok(result),
            Err(_) => Err(BoardClientError::WaitOutcomeUnknown { actor, filter }),
        }
    }
    pub async fn board_inbox_fetch(
        &self,
        request: InboxFetchRequest,
    ) -> Result<InboxFetchResult, BoardClientError> {
        if request.read_mode == InboxReadMode::Latest {
            return self.board_call("board_inbox_fetch", request).await;
        }
        let resource = match &request.scope {
            InboxScope::Project { project_id } => ResourceIdentity::Project {
                project_id: project_id.clone(),
            },
            InboxScope::Board { board_id } => ResourceIdentity::Board {
                board_id: board_id.clone(),
            },
            InboxScope::Topic { topic_id } => ResourceIdentity::Topic {
                topic_id: topic_id.clone(),
            },
        };
        self.board_mutation_call("board_inbox_fetch", request, resource)
            .await
    }
    pub async fn board_inbox_acknowledge(
        &self,
        request: InboxAcknowledgeRequest,
    ) -> Result<InboxAcknowledgeResult, BoardClientError> {
        let resource = match &request.scope {
            ReadScope::Topic { topic_id } => ResourceIdentity::Topic {
                topic_id: topic_id.clone(),
            },
            ReadScope::Thread { root_message_id } => ResourceIdentity::Thread {
                root_message_id: root_message_id.clone(),
            },
        };
        self.board_mutation_call("board_inbox_acknowledge", request, resource)
            .await
    }
    pub async fn board_inbox_projects(
        &self,
        request: InboxProjectsRequest,
    ) -> Result<InboxProjectsResult, BoardClientError> {
        self.board_call("board_inbox_projects", request).await
    }
    async fn board_mutation_call<
        TRequest: serde::Serialize,
        TResult: serde::de::DeserializeOwned,
    >(
        &self,
        method: &str,
        request: TRequest,
        resource: ResourceIdentity,
    ) -> Result<TResult, BoardClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid board request"))?;
        let result = match self.connection.call(method, params).await {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                return Err(BoardClientError::Rejected(Box::new(
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid board failure"))?,
                )));
            }
            Err(error @ ClientError::Rejected { .. }) => return Err(error.into()),
            Err(_) => return Err(outcome_unknown(resource)),
        };
        serde_json::from_value(result).map_err(|_| outcome_unknown(resource))
    }
    async fn board_call<TRequest: serde::Serialize, TResult: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        request: TRequest,
    ) -> Result<TResult, BoardClientError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::Protocol("invalid board request"))?;
        let result = match self.connection.call(method, params).await {
            Ok(result) => result,
            Err(ClientError::Rejected {
                code: -32050,
                data: Some(data),
            }) => {
                return Err(BoardClientError::Rejected(Box::new(
                    serde_json::from_value(data)
                        .map_err(|_| ClientError::Protocol("invalid board failure"))?,
                )));
            }
            Err(error) => return Err(error.into()),
        };
        serde_json::from_value(result)
            .map_err(|_| ClientError::Protocol("invalid board result").into())
    }
}

fn outcome_unknown(resource: ResourceIdentity) -> BoardClientError {
    BoardClientError::OutcomeUnknown {
        resource,
        message: "Board write outcome is unknown. Inspect the affected resource before deciding whether to retry.",
        next_action: BoardNextAction::InspectResource,
    }
}

fn subscription_resource(scope: &SubscriptionScope) -> ResourceIdentity {
    match scope {
        SubscriptionScope::Thread { root_message_id } => ResourceIdentity::Thread {
            root_message_id: root_message_id.clone(),
        },
        SubscriptionScope::Topic { topic_id } => ResourceIdentity::Topic {
            topic_id: topic_id.clone(),
        },
    }
}
