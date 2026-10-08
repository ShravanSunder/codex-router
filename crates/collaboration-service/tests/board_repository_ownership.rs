//! A board request naming a local repository of another Router is refused before any read or
//! write, naming `repository.serviceId`, whichever request carries the repository.
use collaboration_service::ServiceIdentity;
use message_board::*;
use message_board_storage::BoardStore;
use serde_json::{Value, json};
use std::sync::Arc;
mod board_control_support;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const FOREIGN_SERVICE_ID: &str = "00000000-0000-4000-8000-000000000099";

#[tokio::test]
async fn foreign_local_repositories_are_refused_on_every_request_that_names_one()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a project, and requests that each name another Router's local repository.
    let path = std::env::temp_dir().join(format!(
        "repository-ownership-{}.sqlite",
        MessageId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(BoardStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        SERVICE_ID,
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_board_store(Arc::clone(&store));
    let actor = json!({"kind":"human","humanId":"owner"});
    let project_id = ProjectId::generate();
    let foreign =
        json!({"kind":"local","serviceId":FOREIGN_SERVICE_ID,"commonDirectory":"/work/repo"});
    let requests = vec![
        json!({"jsonrpc":"2.0","id":"create","method":"board/projectCreate","params":{"projectId":project_id,"name":"Project","description":"","actor":actor}}),
        json!({"jsonrpc":"2.0","id":"list","method":"board/projectList","params":{"repository":foreign,"page":{"limit":10,"cursor":null}}}),
        json!({"jsonrpc":"2.0","id":"attach","method":"board/repositoryAttach","params":{"projectId":project_id,"repository":foreign,"actor":actor}}),
        json!({"jsonrpc":"2.0","id":"detach","method":"board/repositoryDetach","params":{"projectId":project_id,"repository":foreign,"actor":actor}}),
    ];

    // Act.
    let responses = board_control_support::send_raw_board_requests(identity, requests).await?;

    // Assert: the refusal BoardError::invalid_field gives for repository.serviceId.
    let message = "Correct repository.serviceId: must match the selected board service.";
    let expected = json!({
        "code": -32050,
        "message": message,
        "data": {
            "kind": "invalidField",
            "stage": "validation",
            "message": message,
            "nextAction": "correctRequest",
            "details": {
                "kind": "fieldConstraint",
                "field": "repository.serviceId",
                "requirement": "must match the selected board service"
            }
        }
    });
    if responses
        .first()
        .and_then(|response| response.get("result"))
        .is_none()
    {
        return Err(format!("project creation failed: {responses:?}").into());
    }
    for response in responses.iter().skip(1) {
        if response.get("error") != Some(&expected) {
            return Err(format!("unexpected foreign repository response: {response}").into());
        }
        if response
            .get("result")
            .is_some_and(|result| result != &Value::Null)
        {
            return Err(format!("foreign repository request succeeded: {response}").into());
        }
    }
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
