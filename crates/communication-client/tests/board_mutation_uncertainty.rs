#![allow(clippy::unwrap_used)]

use communication_client::{BoardClientError, ClientError, ControlClient};
use project_board::{
    ActingForIdentity, Description, HumanId, Identity, ProjectCreateRequest, ProjectId,
    ResourceIdentity, ResourceName,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn response_loss_after_board_write_transmission_returns_typed_inspection_identity() {
    let project_id = ProjectId::generate();
    let (mut client, peer) = initialized_client().await;
    let expected_project_id = project_id.clone();
    let server = tokio::spawn(async move {
        let mut peer = peer;
        let request = read_request(&mut peer).await;
        assert_eq!(
            request.get("method").and_then(Value::as_str),
            Some("board/projectCreate")
        );
        assert_eq!(
            request.pointer("/params/projectId").and_then(Value::as_str),
            Some(expected_project_id.as_str())
        );
        // The complete write frame was consumed. Drop without replying.
    });

    let error = client
        .board_project_create(project_create_request(project_id.clone()))
        .await
        .unwrap_err();
    server.await.unwrap();

    match error {
        BoardClientError::OutcomeUnknown {
            resource,
            message,
            next_action,
        } => {
            assert_eq!(resource, ResourceIdentity::Project { project_id });
            assert_eq!(next_action, project_board::BoardNextAction::InspectResource);
            assert!(message.contains("Inspect"));
        }
        other => panic!("expected typed uncertain mutation outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn retired_connection_rejects_mutation_before_transmission_without_claiming_uncertainty() {
    let (mut client, mut peer) = initialized_client().await;
    let server = tokio::spawn(async move {
        let read = read_request(&mut peer).await;
        assert_eq!(
            read.get("method").and_then(Value::as_str),
            Some("board/projectShow")
        );
        peer.get_mut()
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":"wrong-response-id","result":{}})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        // A preflight-rejected second call must never reach this peer.
        let mut unexpected = String::new();
        let read = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            peer.read_line(&mut unexpected),
        )
        .await;
        assert!(read.is_err() || unexpected.is_empty());
    });
    let read_error = client
        .board_project_show(project_board::ProjectShowRequest {
            project_id: ProjectId::generate(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        read_error,
        BoardClientError::Connection(ClientError::Protocol("response ID mismatch"))
    ));

    let mutation_error = client
        .board_project_create(project_create_request(ProjectId::generate()))
        .await
        .unwrap_err();
    assert!(matches!(
        mutation_error,
        BoardClientError::Connection(ClientError::Protocol("connection is retired"))
    ));
    server.await.unwrap();
}

async fn initialized_client() -> (ControlClient, BufReader<tokio::net::UnixStream>) {
    let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
    let initialize = tokio::spawn(async move {
        ControlClient::initialize(client_stream, "board-uncertainty-test", "1")
            .await
            .unwrap()
    });
    let mut peer = BufReader::new(server_stream);
    let request = read_request(&mut peer).await;
    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some("control/initialize")
    );
    let response = json!({
        "jsonrpc":"2.0",
        "id":request.get("id").cloned().unwrap(),
        "result":{
            "version":{"major":1,"minor":0},
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "serviceEpoch":"00000000-0000-4000-8000-000000000002",
            "controlSchemaDigest":format!("sha256:{}", "a".repeat(64)),
        }
    });
    peer.get_mut()
        .write_all(format!("{response}\n").as_bytes())
        .await
        .unwrap();
    (initialize.await.unwrap(), peer)
}

async fn read_request(peer: &mut BufReader<tokio::net::UnixStream>) -> Value {
    let mut line = String::new();
    peer.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

fn project_create_request(project_id: ProjectId) -> ProjectCreateRequest {
    ProjectCreateRequest {
        project_id,
        name: ResourceName::try_from("Uncertain SDK write".to_owned()).unwrap(),
        description: Description::try_from(String::new()).unwrap(),
        actor: Identity::Human {
            human_id: HumanId::try_from("sdk-uncertainty".to_owned()).unwrap(),
        },
        acting_for: Some(ActingForIdentity::Human {
            human_id: HumanId::try_from("owner".to_owned()).unwrap(),
        }),
    }
}
