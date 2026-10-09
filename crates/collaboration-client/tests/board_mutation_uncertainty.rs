#![allow(clippy::unwrap_used)]

use collaboration_client::{BoardClientError, CollaborationClient};
use message_board::{
    ActingForIdentity, Description, HumanId, Identity, ProjectCreateRequest, ProjectId,
    ResourceIdentity, ResourceName,
};
use serde_json::Value;

#[path = "support/scripted_api.rs"]
mod scripted_api;
use scripted_api::ScriptedApi;

#[tokio::test]
async fn response_loss_after_board_write_transmission_returns_typed_inspection_identity() {
    let project_id = ProjectId::generate();
    let (api, mut calls) = ScriptedApi::start().await.unwrap();
    let client = CollaborationClient::connect(api.directory(), "board-uncertainty-test", "1")
        .await
        .unwrap();
    let expected_project_id = project_id.clone();
    let server = tokio::spawn(async move {
        let request = calls.next().await.unwrap();
        assert_eq!(request.tool, "board_project_create");
        assert_eq!(
            request
                .arguments
                .pointer("/projectId")
                .and_then(Value::as_str),
            Some(expected_project_id.as_str())
        );
        // The complete write was received. Close without replying.
        request.lose_response();
        calls
    });

    let error = client
        .board_project_create(project_create_request(project_id.clone()))
        .await
        .unwrap_err();
    let mut calls = server.await.unwrap();

    match error {
        BoardClientError::OutcomeUnknown {
            resource,
            message,
            next_action,
        } => {
            assert_eq!(resource, ResourceIdentity::Project { project_id });
            assert_eq!(next_action, message_board::BoardNextAction::InspectResource);
            assert!(message.contains("Inspect"));
        }
        other => panic!("expected typed uncertain mutation outcome, got {other:?}"),
    }
    // The uncertain write is never replayed.
    assert!(
        calls
            .none_within(std::time::Duration::from_millis(100))
            .await
    );
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
