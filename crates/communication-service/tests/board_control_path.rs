use communication_client::{BoardClientError, ControlClient};
use communication_service::{ServiceIdentity, serve_control_connection};
use project_board::*;
use project_board_storage::BoardStore;
use std::sync::Arc;
mod board_control_support;

#[tokio::test]
async fn board_control_roundtrip_preserves_root_thread_and_actor()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "board-control-{}.sqlite",
        MessageId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(BoardStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_board_store(store.clone());
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "board-test", "1").await?;
    let actor = Identity::Human {
        human_id: HumanId::try_from("reader-one".to_owned())?,
    };
    let project_id = ProjectId::generate();
    client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Project".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let board_id = BoardId::generate();
    client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: ResourceName::try_from("Board".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let topic_id = TopicId::generate();
    client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("Topic".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let root = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic { topic_id },
            actor: actor.clone(),
            acting_for: None,
            text: MessageText::try_from("Main".to_owned())?,
            references: vec![].try_into()?,
        })
        .await?;
    if !root.watch_status.watching {
        return Err("root post did not activate watch".into());
    }
    let reply = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            actor: actor.clone(),
            acting_for: None,
            text: MessageText::try_from("Thread".to_owned())?,
            references: vec![ReferenceTarget::Message {
                message_id: root.message.message_id.clone(),
            }]
            .try_into()?,
        })
        .await?;
    if reply.message.actor != actor {
        return Err("thread actor was not preserved".into());
    }
    let thread = client
        .board_thread_show(ThreadShowRequest {
            root_message_id: root.message.message_id,
            reader: None,
        })
        .await?;
    if thread.thread.state != ThreadState::Unresolved {
        return Err("new thread was not unresolved".into());
    }
    drop(client);
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn malformed_board_requests_return_safe_specific_failures()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "board-control-validation-{}.sqlite",
        MessageId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(BoardStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_board_store(store.clone());
    let valid_id = MessageId::generate();
    let actor = serde_json::json!({"kind":"human","humanId":"reader"});
    let private_value = "PRIVATE_REQUEST_VALUE_MUST_NOT_ECHO";
    let responses = board_control_support::send_raw_board_requests(
        identity,
        vec![
            serde_json::json!({"jsonrpc":"2.0","id":"actor","method":"board/projectCreate","params":{
                "projectId":valid_id,"name":"Project","description":"","actor":{"kind":"futureSecret","private":private_value}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"reader","method":"board/inboxFetch","params":{
                "projectId":valid_id,"reader":null,"page":{"limit":50,"cursor":null}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"actingFor","method":"board/projectCreate","params":{
                "projectId":valid_id,"name":"Project","description":"","actor":actor,
                "actingFor":{"kind":"session","session":{"private":private_value}}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"topicName","method":"board/topicCreate","params":{
                "topicId":valid_id,"boardId":valid_id,"name":"   ","description":"","actor":actor
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"uuid","method":"board/projectShow","params":{"projectId":"not-a-uuid"}}),
            serde_json::json!({"jsonrpc":"2.0","id":"description","method":"board/projectCreate","params":{
                "projectId":valid_id,"name":"Project","description":"x".repeat(16_385),"actor":actor
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"name","method":"board/projectCreate","params":{
                "projectId":valid_id,"name":"n".repeat(257),"description":"","actor":actor
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"text","method":"board/messagePost","params":{
                "messageId":valid_id,"placement":{"kind":"topic","topicId":valid_id},
                "actor":actor,"text":"t".repeat(65_537),"references":[]
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"page","method":"board/projectList","params":{
                "repository":null,"page":{"limit":101,"cursor":null}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"position","method":"board/messageList","params":{
                "scope":{"kind":"topic","topicId":valid_id},
                "selection":{"kind":"afterPosition","afterActivitySequence":9_223_372_036_854_775_808_u64},
                "page":{"limit":50,"cursor":null}
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"placement","method":"board/messagePost","params":{
                "messageId":valid_id,"placement":{"kind":"privateKind","private":private_value},
                "actor":actor,"text":"message","references":[]
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":"extra","method":"board/projectShow","params":{
                "projectId":valid_id,"privateField":private_value
            }}),
        ],
    )
    .await?;
    let expected = [
        ("invalidIdentity", "actor", "4096"),
        ("invalidIdentity", "reader", "4096"),
        ("invalidIdentity", "actingFor", "4096"),
        ("invalidTopicName", "name", "256"),
        ("invalidField", "projectId", "UUIDv7"),
        ("invalidField", "description", "16384"),
        ("invalidField", "name", "256"),
        ("invalidField", "text", "65536"),
        ("invalidField", "page.limit", "100"),
        (
            "invalidField",
            "selection.afterActivitySequence",
            "9223372036854775807",
        ),
        ("invalidField", "placement", "topic or thread"),
        ("invalidField", "request", "camelCase"),
    ];
    if responses.len() != expected.len() {
        return Err("validation response count changed".into());
    }
    for (response, (kind, field, requirement)) in responses.iter().zip(expected) {
        if response
            .pointer("/error/data/kind")
            .and_then(serde_json::Value::as_str)
            != Some(kind)
            || response
                .pointer("/error/data/details/field")
                .and_then(serde_json::Value::as_str)
                != Some(field)
            || !response
                .pointer("/error/data/details/requirement")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.contains(requirement))
        {
            return Err(format!("unexpected classified board failure: {response}").into());
        }
        let encoded = response.to_string();
        if encoded.contains(private_value)
            || encoded.contains("privateField")
            || encoded.contains("privateKind")
        {
            return Err(
                "validation response echoed a supplied value, unknown key, or private variant"
                    .into(),
            );
        }
    }
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn control_board_failures_pagination_and_inbox_use_the_public_path()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "board-control-behavior-{}.sqlite",
        MessageId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(BoardStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_board_store(store.clone());
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "board-behavior-test", "1").await?;
    macro_rules! rejected_on_fresh_connection {
        ($method:ident, $request:expr) => {{
            let (socket, server) = tokio::net::UnixStream::pair()?;
            let temporary_task = tokio::spawn(serve_control_connection(server, identity.clone()));
            let mut temporary_client =
                ControlClient::initialize(socket, "board-rejection-test", "1").await?;
            let result = temporary_client.$method($request).await;
            drop(temporary_client);
            temporary_task.await??;
            rejected_error(result)?
        }};
    }
    let alice = Identity::Human {
        human_id: HumanId::try_from("alice".to_owned())?,
    };
    let bob = Identity::Human {
        human_id: HumanId::try_from("bob".to_owned())?,
    };
    let project_id = ProjectId::generate();
    let project_request = ProjectCreateRequest {
        project_id: project_id.clone(),
        name: ResourceName::try_from("Project".to_owned())?,
        description: Description::try_from(String::new())?,
        actor: alice.clone(),
        acting_for: None,
    };
    client.board_project_create(project_request.clone()).await?;
    if rejected_on_fresh_connection!(board_project_create, project_request).kind
        != BoardFailureKind::ResourceAlreadyExists
    {
        return Err("duplicate project did not return resourceAlreadyExists".into());
    }
    let board_id = BoardId::generate();
    client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id: project_id.clone(),
            name: ResourceName::try_from("Board".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: alice.clone(),
            acting_for: None,
        })
        .await?;
    let topic_id = TopicId::generate();
    client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id: board_id.clone(),
            name: ResourceName::try_from("Topic".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: alice.clone(),
            acting_for: None,
        })
        .await?;
    if rejected_on_fresh_connection!(
        board_topic_create,
        TopicCreateRequest {
            topic_id: TopicId::generate(),
            board_id: board_id.clone(),
            name: ResourceName::try_from("Topic".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: alice.clone(),
            acting_for: None,
        }
    )
    .kind
        != BoardFailureKind::InvalidTopicName
    {
        return Err("duplicate topic name did not return invalidTopicName".into());
    }
    client
        .board_inbox_fetch(InboxFetchRequest {
            project_id: project_id.clone(),
            reader: bob.clone(),
            page: PageRequest::default(),
        })
        .await?;
    let root = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic {
                topic_id: topic_id.clone(),
            },
            actor: alice.clone(),
            acting_for: None,
            text: MessageText::try_from("Root".to_owned())?,
            references: vec![].try_into()?,
        })
        .await?;
    let cooldown = rejected_on_fresh_connection!(
        board_message_post,
        MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic {
                topic_id: topic_id.clone(),
            },
            actor: alice.clone(),
            acting_for: None,
            text: MessageText::try_from("Too soon".to_owned())?,
            references: vec![].try_into()?,
        }
    );
    if cooldown.kind != BoardFailureKind::TopLevelMessageCooldown
        || !matches!(cooldown.details, BoardErrorDetails::Cooldown { retry_after_seconds } if (1..=30).contains(&retry_after_seconds))
    {
        return Err("Control cooldown failure omitted its actionable remaining duration".into());
    }
    client
        .board_thread_watch(ThreadWatchRequest {
            root_message_id: root.message.message_id.clone(),
            actor: bob.clone(),
            acting_for: None,
        })
        .await?;
    let reply = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            actor: alice.clone(),
            acting_for: None,
            text: MessageText::try_from("Reply".to_owned())?,
            references: vec![ReferenceTarget::Message {
                message_id: root.message.message_id.clone(),
            }]
            .try_into()?,
        })
        .await?;
    let inbox = client
        .board_inbox_fetch(InboxFetchRequest {
            project_id: project_id.clone(),
            reader: bob.clone(),
            page: PageRequest::default(),
        })
        .await?;
    if !inbox.page.records.iter().any(|activity| matches!(activity, InboxActivity::MessageCreated { message, .. } if message.message_id == reply.message.message_id)) {
        return Err("Control inbox did not expose the watched thread reply".into());
    }
    let acknowledgement = client
        .board_inbox_acknowledge(InboxAcknowledgeRequest {
            actor: bob.clone(),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            through_activity_sequence: reply.message.activity_sequence,
        })
        .await?;
    if !acknowledgement.project_unread_summary.has_unread {
        return Err(
            "Thread acknowledgement incorrectly cleared the independent topic scope".into(),
        );
    }
    let topic_acknowledgement = client
        .board_inbox_acknowledge(InboxAcknowledgeRequest {
            actor: bob.clone(),
            acting_for: None,
            scope: ReadScope::Topic {
                topic_id: topic_id.clone(),
            },
            through_activity_sequence: root.message.activity_sequence,
        })
        .await?;
    if topic_acknowledgement.project_unread_summary.has_unread {
        return Err("Control acknowledgements did not clear both processed scopes".into());
    }
    client
        .board_thread_resolve(ThreadResolveRequest {
            root_message_id: root.message.message_id.clone(),
            actor: alice.clone(),
            acting_for: None,
        })
        .await?;
    let resolved = rejected_on_fresh_connection!(
        board_message_post,
        MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            actor: bob.clone(),
            acting_for: None,
            text: MessageText::try_from("Blocked".to_owned())?,
            references: vec![].try_into()?,
        }
    );
    if resolved.kind != BoardFailureKind::ThreadResolved
        || resolved.message
            != "This thread is resolved. Mark it unresolved before adding a thread message."
    {
        return Err("Control resolved-thread failure mapping changed".into());
    }
    client
        .board_thread_unresolve(ThreadUnresolveRequest {
            root_message_id: root.message.message_id.clone(),
            actor: alice.clone(),
            acting_for: None,
        })
        .await?;
    let escaped_body = "\\\"".repeat(32_768);
    for _index in 0..9 {
        client
            .board_message_post(MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root.message.message_id.clone(),
                },
                actor: alice.clone(),
                acting_for: None,
                text: MessageText::try_from(escaped_body.clone())?,
                references: vec![].try_into()?,
            })
            .await?;
    }
    let mut cursor = None;
    let mut observed_messages = 0;
    let mut pages = 0;
    loop {
        let result = client
            .board_message_list(MessageListRequest {
                scope: MessageListScope::Board {
                    board_id: board_id.clone(),
                },
                selection: MessageSelection::Latest,
                page: PageRequest {
                    limit: 100.try_into()?,
                    cursor,
                },
            })
            .await?;
        if serde_json::to_vec(&result)?.len() >= communication_protocol::MAX_CONTROL_FRAME_BYTES {
            return Err("Control message page exceeded its frame budget".into());
        }
        observed_messages += result.page.records.len();
        pages += 1;
        cursor = result.page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    if observed_messages != 11 || pages < 2 {
        return Err("Control pagination skipped messages or ignored its byte budget".into());
    }
    client
        .board_archive(BoardArchiveRequest {
            board_id,
            actor: alice.clone(),
            acting_for: None,
        })
        .await?;
    if rejected_on_fresh_connection!(
        board_message_post,
        MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.message.message_id,
            },
            actor: alice,
            acting_for: None,
            text: MessageText::try_from("Archived".to_owned())?,
            references: vec![].try_into()?,
        }
    )
    .kind
        != BoardFailureKind::ArchivedBoard
    {
        return Err("archived board post did not return archivedBoard".into());
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}

fn rejected_error<TValue>(
    result: Result<TValue, BoardClientError>,
) -> Result<BoardError, Box<dyn std::error::Error>> {
    match result {
        Err(BoardClientError::Rejected(error)) => Ok(*error),
        Err(error) => Err(error.into()),
        Ok(_) => Err("board operation unexpectedly succeeded".into()),
    }
}
