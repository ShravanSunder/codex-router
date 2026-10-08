use collaboration_service::ServiceIdentity;
use message_board::*;
use message_board_storage::BoardStore;
use std::sync::Arc;
mod board_control_support;
use board_control_support::served_api;

#[tokio::test]
async fn oversized_repository_locators_name_field_and_numeric_bound()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "repository-control-{}.sqlite",
        MessageId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(BoardStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
    )
    .map_err(std::io::Error::other)?
    .with_board_store(store.clone());
    let actor = serde_json::json!({"kind":"human","humanId":"owner"});
    let project_id = ProjectId::generate();
    let responses = board_control_support::send_raw_board_requests(identity, vec![
        serde_json::json!({"jsonrpc":"2.0","id":"origin","method":"board/repositoryAttach","params":{"projectId":project_id,"repository":{"kind":"origin","normalizedOrigin":format!("github.com/{}", "x".repeat(455_000))},"actor":actor}}),
        serde_json::json!({"jsonrpc":"2.0","id":"directory","method":"board/repositoryAttach","params":{"projectId":project_id,"repository":{"kind":"local","serviceId":"00000000-0000-4000-8000-000000000001","commonDirectory":format!("/{}", "x".repeat(500_000))},"actor":actor}}),
    ]).await?;
    // Tool arguments that fail to decode are refused at validation with the decoder's
    // message, which names the field and its bound; the oversized value is not echoed.
    for (response, field) in responses
        .iter()
        .zip(["normalizedOrigin", "commonDirectory"])
    {
        let failure = response.pointer("/error/data");
        let text = |name: &str| {
            failure
                .and_then(|failure| failure.get(name))
                .and_then(serde_json::Value::as_str)
        };
        let valid = text("kind") == Some("protocolViolation")
            && text("stage") == Some("validation")
            && text("effect") == Some("none")
            && text("message").is_some_and(|message| message.contains(field))
            && text("message").is_some_and(|message| message.contains("4096"))
            && response.to_string().len() < 1_048_576;
        if !valid {
            return Err(
                format!("unexpected repository locator validation response: {response}").into(),
            );
        }
    }
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
    )
    .map_err(std::io::Error::other)?
    .with_board_store(store.clone());
    let served = served_api::ServedApi::start(identity).await?;
    let client = served.client("repository-boundary-test").await?;
    let actor = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned())?,
    };
    client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Repository project".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    for marker in ['a', 'b'] {
        let escaped_path = format!("/{marker}{}", "\\\"".repeat(2_047));
        client
            .board_repository_attach(RepositoryAttachRequest {
                project_id: project_id.clone(),
                repository: RepositoryRef::Local {
                    service_id: ServiceId::try_from(
                        "00000000-0000-4000-8000-000000000001".to_owned(),
                    )?,
                    common_directory: CommonDirectory::try_from(escaped_path)?,
                },
                actor: actor.clone(),
                acting_for: None,
            })
            .await?;
    }
    let first = client
        .board_repository_list(RepositoryListRequest {
            project_id: project_id.clone(),
            page: PageRequest {
                limit: PageLimit::try_from(1)?,
                cursor: None,
            },
        })
        .await?;
    if first.page.records.len() != 1 || serde_json::to_vec(&first)?.len() >= 1_048_576 {
        return Err("first max-bound repository page exceeded the frame or row limit".into());
    }
    let second = client
        .board_repository_list(RepositoryListRequest {
            project_id,
            page: PageRequest {
                limit: PageLimit::try_from(1)?,
                cursor: first.page.next_cursor,
            },
        })
        .await?;
    if second.page.records.len() != 1
        || second.page.next_cursor.is_some()
        || serde_json::to_vec(&second)?.len() >= 1_048_576
    {
        return Err("second max-bound repository page exceeded the frame or did not finish".into());
    }
    served.stop().await?;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
