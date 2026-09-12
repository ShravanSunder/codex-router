//! Typed board calls share Control transport and never replay uncertain writes.
use crate::{ClientError, ControlClient};
use project_board::*;
#[derive(Debug, thiserror::Error)]
pub enum BoardClientError {
    #[error("{0}")]
    Rejected(Box<BoardError>),
    #[error(transparent)]
    Connection(#[from] ClientError),
}
impl ControlClient {
    pub async fn board_project_create(
        &mut self,
        request: ProjectCreateRequest,
    ) -> Result<ProjectCreateResult, BoardClientError> {
        self.board_call("board/projectCreate", request).await
    }
    pub async fn board_project_update(
        &mut self,
        request: ProjectUpdateRequest,
    ) -> Result<ProjectUpdateResult, BoardClientError> {
        self.board_call("board/projectUpdate", request).await
    }
    pub async fn board_project_show(
        &mut self,
        request: ProjectShowRequest,
    ) -> Result<ProjectShowResult, BoardClientError> {
        self.board_call("board/projectShow", request).await
    }
    pub async fn board_project_list(
        &mut self,
        request: ProjectListRequest,
    ) -> Result<ProjectListResult, BoardClientError> {
        self.board_call("board/projectList", request).await
    }
    pub async fn board_repository_attach(
        &mut self,
        request: RepositoryAttachRequest,
    ) -> Result<RepositoryAttachResult, BoardClientError> {
        self.board_call("board/repositoryAttach", request).await
    }
    pub async fn board_repository_detach(
        &mut self,
        request: RepositoryDetachRequest,
    ) -> Result<RepositoryDetachResult, BoardClientError> {
        self.board_call("board/repositoryDetach", request).await
    }
    pub async fn board_repository_list(
        &mut self,
        request: RepositoryListRequest,
    ) -> Result<RepositoryListResult, BoardClientError> {
        self.board_call("board/repositoryList", request).await
    }
    pub async fn board_create(
        &mut self,
        request: BoardCreateRequest,
    ) -> Result<BoardCreateResult, BoardClientError> {
        self.board_call("board/create", request).await
    }
    pub async fn board_update(
        &mut self,
        request: BoardUpdateRequest,
    ) -> Result<BoardUpdateResult, BoardClientError> {
        self.board_call("board/update", request).await
    }
    pub async fn board_show(
        &mut self,
        request: BoardShowRequest,
    ) -> Result<BoardShowResult, BoardClientError> {
        self.board_call("board/show", request).await
    }
    pub async fn board_list(
        &mut self,
        request: BoardListRequest,
    ) -> Result<BoardListResult, BoardClientError> {
        self.board_call("board/list", request).await
    }
    pub async fn board_archive(
        &mut self,
        request: BoardArchiveRequest,
    ) -> Result<BoardArchiveResult, BoardClientError> {
        self.board_call("board/archive", request).await
    }
    pub async fn board_topic_create(
        &mut self,
        request: TopicCreateRequest,
    ) -> Result<TopicCreateResult, BoardClientError> {
        self.board_call("board/topicCreate", request).await
    }
    pub async fn board_topic_update(
        &mut self,
        request: TopicUpdateRequest,
    ) -> Result<TopicUpdateResult, BoardClientError> {
        self.board_call("board/topicUpdate", request).await
    }
    pub async fn board_topic_list(
        &mut self,
        request: TopicListRequest,
    ) -> Result<TopicListResult, BoardClientError> {
        self.board_call("board/topicList", request).await
    }
    pub async fn board_message_post(
        &mut self,
        request: MessagePostRequest,
    ) -> Result<MessagePostResult, BoardClientError> {
        self.board_call("board/messagePost", request).await
    }
    pub async fn board_message_show(
        &mut self,
        request: MessageShowRequest,
    ) -> Result<MessageShowResult, BoardClientError> {
        self.board_call("board/messageShow", request).await
    }
    pub async fn board_message_list(
        &mut self,
        request: MessageListRequest,
    ) -> Result<MessageListResult, BoardClientError> {
        self.board_call("board/messageList", request).await
    }
    pub async fn board_thread_show(
        &mut self,
        request: ThreadShowRequest,
    ) -> Result<ThreadShowResult, BoardClientError> {
        self.board_call("board/threadShow", request).await
    }
    pub async fn board_thread_resolve(
        &mut self,
        request: ThreadResolveRequest,
    ) -> Result<ThreadResolveResult, BoardClientError> {
        self.board_call("board/threadResolve", request).await
    }
    pub async fn board_thread_unresolve(
        &mut self,
        request: ThreadUnresolveRequest,
    ) -> Result<ThreadUnresolveResult, BoardClientError> {
        self.board_call("board/threadUnresolve", request).await
    }
    pub async fn board_thread_watch(
        &mut self,
        request: ThreadWatchRequest,
    ) -> Result<ThreadWatchResult, BoardClientError> {
        self.board_call("board/threadWatch", request).await
    }
    pub async fn board_thread_unwatch(
        &mut self,
        request: ThreadUnwatchRequest,
    ) -> Result<ThreadUnwatchResult, BoardClientError> {
        self.board_call("board/threadUnwatch", request).await
    }
    pub async fn board_thread_list(
        &mut self,
        request: ThreadListRequest,
    ) -> Result<ThreadListResult, BoardClientError> {
        self.board_call("board/threadList", request).await
    }
    pub async fn board_inbox_fetch(
        &mut self,
        request: InboxFetchRequest,
    ) -> Result<InboxFetchResult, BoardClientError> {
        self.board_call("board/inboxFetch", request).await
    }
    pub async fn board_inbox_acknowledge(
        &mut self,
        request: InboxAcknowledgeRequest,
    ) -> Result<InboxAcknowledgeResult, BoardClientError> {
        self.board_call("board/inboxAcknowledge", request).await
    }
    pub async fn board_inbox_projects(
        &mut self,
        request: InboxProjectsRequest,
    ) -> Result<InboxProjectsResult, BoardClientError> {
        self.board_call("board/inboxProjects", request).await
    }
    async fn board_call<TRequest: serde::Serialize, TResult: serde::de::DeserializeOwned>(
        &mut self,
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
