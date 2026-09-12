//! Board-family Control dispatch; typed domain requests enter one serialized store.
use crate::ServiceIdentity;
use project_board::*;
use serde_json::{Value, json};
pub(crate) fn failure(id: Value, error: BoardError) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":error.message,"data":error}})
}
pub(crate) fn unavailable(id: Value) -> Value {
    failure(
        id,
        BoardError {
            kind: BoardFailureKind::BoardUnavailable,
            stage: BoardFailureStage::Admission,
            message: "Board storage unavailable; inspect the selected debug/service profile."
                .into(),
            next_action: BoardNextAction::RetryLater,
            details: BoardErrorDetails::None,
        },
    )
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
    let Some(store) = identity.board.as_ref() else {
        return unavailable(id);
    };
    if let Some(repository) = params.get("repository")
        && let Ok(RepositoryRef::Local { service_id, .. }) =
            serde_json::from_value::<RepositoryRef>(repository.clone())
        && service_id.as_str() != String::from(identity.service_id.clone())
    {
        return failure(id, BoardError { kind: BoardFailureKind::InvalidField, stage: BoardFailureStage::Validation, message: "Local repository reference belongs to another service. Select the owning service.".into(), next_action: BoardNextAction::CorrectRequest, details: BoardErrorDetails::None });
    }

    macro_rules! call {
 ($request:ty,$method:ident)=>{{
 let request=match serde_json::from_value::<$request>(params) { Ok(request)=>request,Err(_error)=>return failure(id,BoardError{kind:BoardFailureKind::InvalidField,stage:BoardFailureStage::Validation,message:"Invalid board request. Check required fields, identity variants and value bounds.".into(),next_action:BoardNextAction::CorrectRequest,details:BoardErrorDetails::None}) };
 let result=store.lock().await.$method(request).await;
 match result { Ok(result)=>json!({"jsonrpc":"2.0","id":id,"result":result}),Err(error)=>failure(id,error) }
 }};
 }
    match method {
        "board/projectCreate" => call!(ProjectCreateRequest, create_project),
        "board/projectUpdate" => call!(ProjectUpdateRequest, update_project),
        "board/projectShow" => call!(ProjectShowRequest, show_project),
        "board/projectList" => call!(ProjectListRequest, list_projects),
        "board/repositoryAttach" => call!(RepositoryAttachRequest, attach_repository),
        "board/repositoryDetach" => call!(RepositoryDetachRequest, detach_repository),
        "board/repositoryList" => call!(RepositoryListRequest, list_repositories),
        "board/create" => call!(BoardCreateRequest, create_board),
        "board/update" => call!(BoardUpdateRequest, update_board),
        "board/show" => call!(BoardShowRequest, show_board),
        "board/list" => call!(BoardListRequest, list_boards),
        "board/archive" => call!(BoardArchiveRequest, archive_board),
        "board/topicCreate" => call!(TopicCreateRequest, create_topic),
        "board/topicUpdate" => call!(TopicUpdateRequest, update_topic),
        "board/topicList" => call!(TopicListRequest, list_topics),
        "board/messagePost" => call!(MessagePostRequest, post_message),
        "board/messageShow" => call!(MessageShowRequest, show_message),
        "board/messageList" => call!(MessageListRequest, list_messages),
        "board/threadShow" => call!(ThreadShowRequest, show_thread),
        "board/threadResolve" => call!(ThreadResolveRequest, resolve_thread),
        "board/threadUnresolve" => call!(ThreadUnresolveRequest, unresolve_thread),
        "board/threadWatch" => call!(ThreadWatchRequest, watch_thread),
        "board/threadUnwatch" => call!(ThreadUnwatchRequest, unwatch_thread),
        "board/threadList" => call!(ThreadListRequest, list_threads),
        "board/inboxFetch" => call!(InboxFetchRequest, fetch_inbox),
        "board/inboxAcknowledge" => call!(InboxAcknowledgeRequest, acknowledge_inbox),
        "board/inboxProjects" => call!(InboxProjectsRequest, list_inbox_projects),
        _ => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown board method"}})
        }
    }
}
