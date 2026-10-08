//! Board-family Control dispatch: decodes each request and calls the typed board operations.
use crate::ServiceIdentity;
use crate::collaboration_application::BoardOperations;
use message_board::*;
use serde_json::{Value, json};
pub(crate) fn failure(id: Value, error: BoardError) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":error.message,"data":error}})
}
pub(crate) fn overloaded(id: Value) -> Value {
    failure(
        id,
        BoardError {
            kind: BoardFailureKind::Overloaded,
            stage: BoardFailureStage::Admission,
            message: "Request capacity exceeded; no board mutation was dispatched. Retry later."
                .into(),
            next_action: BoardNextAction::RetryLater,
            details: BoardErrorDetails::None,
        },
    )
}
pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let board = BoardOperations::new(identity);
    macro_rules! call {
        ($request:ty, $operation:ident) => {{
            let request = match serde_json::from_value::<$request>(params.clone()) {
                Ok(request) => request,
                Err(_error) => {
                    return failure(
                        id,
                        crate::board_request_validation::classify(method, &params),
                    );
                }
            };
            match board.$operation(request).await {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(error) => failure(id, error),
            }
        }};
    }
    match method {
        "board/threadSubscribe"
        | "board/threadUnsubscribe"
        | "board/threadSubscriptions"
        | "board/threadWait" => {
            crate::thread_subscription_dispatch::dispatch(id, method, params, identity).await
        }
        "board/discoverySearch" => call!(DiscoverySearchRequest, discovery_search),
        "board/messageSearch" => call!(MessageSearchRequest, message_search),
        "board/projectCreate" => call!(ProjectCreateRequest, project_create),
        "board/projectUpdate" => call!(ProjectUpdateRequest, project_update),
        "board/projectShow" => call!(ProjectShowRequest, project_show),
        "board/projectList" => call!(ProjectListRequest, project_list),
        "board/repositoryAttach" => call!(RepositoryAttachRequest, repository_attach),
        "board/repositoryDetach" => call!(RepositoryDetachRequest, repository_detach),
        "board/repositoryList" => call!(RepositoryListRequest, repository_list),
        "board/create" => call!(BoardCreateRequest, board_create),
        "board/update" => call!(BoardUpdateRequest, board_update),
        "board/show" => call!(BoardShowRequest, board_show),
        "board/list" => call!(BoardListRequest, board_list),
        "board/archive" => call!(BoardArchiveRequest, board_archive),
        "board/topicCreate" => call!(TopicCreateRequest, topic_create),
        "board/topicUpdate" => call!(TopicUpdateRequest, topic_update),
        "board/topicList" => call!(TopicListRequest, topic_list),
        "board/messagePost" => call!(MessagePostRequest, message_post),
        "board/messageShow" => call!(MessageShowRequest, message_show),
        "board/messageList" => call!(MessageListRequest, message_list),
        "board/threadShow" => call!(ThreadShowRequest, thread_show),
        "board/threadResolve" => call!(ThreadResolveRequest, thread_resolve),
        "board/threadUnresolve" => call!(ThreadUnresolveRequest, thread_unresolve),
        "board/threadWatch" => call!(ThreadWatchRequest, thread_watch),
        "board/threadUnwatch" => call!(ThreadUnwatchRequest, thread_unwatch),
        "board/topicWatch" => call!(TopicWatchRequest, topic_watch),
        "board/topicUnwatch" => call!(TopicWatchRequest, topic_unwatch),
        "board/threadList" => call!(ThreadListRequest, thread_list),
        "board/threadCreate" => call!(ThreadCreateRequest, thread_create),
        "board/threadJoin" => call!(ThreadJoinRequest, thread_join),
        "board/threadLeave" => call!(ThreadLeaveRequest, thread_leave),
        "board/threadParticipantList" => {
            call!(ThreadParticipantListRequest, thread_participant_list)
        }
        "board/inboxFetch" => call!(InboxFetchRequest, inbox_fetch),
        "board/inboxAcknowledge" => call!(InboxAcknowledgeRequest, inbox_acknowledge),
        "board/inboxProjects" => call!(InboxProjectsRequest, inbox_projects),
        _ => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown board method"}})
        }
    }
}
