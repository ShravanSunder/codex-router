use collaboration_client::ControlClient;
use collaboration_protocol::{EndpointRef, QuestionAnswerParams, QuestionResponse, QuestionState};
use collaboration_service::{
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, ServiceInteractionBroker,
    serve_control_connection,
};
use message_board::{HumanId, Identity, SessionEndpointRef, SessionId, SessionRef};
use serde_json::json;

// R18: the public Control client and service agree on field projection, actor
// authorization, and the exact response sent to the waiting agent.
#[tokio::test]
async fn question_list_and_answer_cross_the_real_control_connection() {
    let root = tempfile::tempdir().expect("temporary service directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let endpoint: EndpointRef = serde_json::from_value(json!({
        "serviceId":service_id,"endpointId":"claude-local"
    }))
    .expect("endpoint");
    let imported = collaboration_service::InteractionHistoryRecord::RefusedApproval {
        requester: serde_json::from_value(json!({"endpoint":{"serviceId":service_id,"endpointId":"claude-local"},"sessionId":"imported-requester"})).expect("imported identity"),
        approver: Identity::Human { human_id: HumanId::try_from("owner".to_owned()).expect("human") },
        refusal: collaboration_service::RefusedTypedApproval { request_id:"imported-refusal".into(),
            title:"Imported refusal".into(), description:None, subject:None, options:vec![], reason:"fixture".into() }
    };
    let original = serde_json::to_vec_pretty(&std::collections::BTreeMap::from([(
        "imported-refusal",
        imported,
    )]))
    .expect("preexisting typed JSON");
    tokio::fs::write(root.path().join("interaction-history.json"), &original)
        .await
        .expect("source before broker load");
    let broker = ServiceInteractionBroker::load(
        service_id.to_owned().try_into().expect("service ID"),
        NativeControlBackend {
            endpoint,
            gate: NativeGenerationGate::default(),
            codex_home: root.path().to_path_buf(),
        },
        root.path().join("approval-routes.json"),
    )
    .await
    .expect("broker");
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .expect("identity")
        .with_approval_broker(broker.clone());
    let requester = SessionRef {
        endpoint: SessionEndpointRef {
            service_id: service_id.to_owned().try_into().expect("service"),
            endpoint_id: "claude-local".to_owned().try_into().expect("endpoint"),
        },
        session_id: SessionId::try_from("provider-session".to_owned()).expect("session"),
    };
    let approver = Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human"),
    };
    let request = serde_json::from_value(json!({
        "requestId":"question-1", "prompt":"Choose launch settings", "fields":[
            {"kind":"number","fieldId":"count","label":"Count","description":null,"required":true},
            {"kind":"boolean","fieldId":"dryRun","label":"Dry run","description":null,"required":true},
            {"kind":"singleChoice","fieldId":"color","label":"Color","description":null,"required":true,"options":[{"optionId":"red","label":"Red"},{"optionId":"blue","label":"Blue"}]}
        ]
    })).expect("question");
    let agent_reply = broker
        .request_question(requester, approver.clone(), request, None)
        .await
        .expect("pending question");
    let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("socket pair");
    let server = tokio::spawn(serve_control_connection(server_stream, identity.clone()));
    let mut client = ControlClient::initialize(client_stream, "question-test", "1")
        .await
        .expect("initialize");
    let list = client.list_questions(true).await.expect("list");
    assert_eq!(list.questions.len(), 1);
    assert_eq!(list.questions[0].prompt, "Choose launch settings");
    assert_eq!(list.questions[0].fields.len(), 3);
    let answer = QuestionResponse::Answered {
        content: serde_json::from_value(
            json!({"count":3,"dryRun":true,"color":{"selectedOptionIds":["blue"]}}),
        )
        .expect("content"),
    };
    let wrong_actor = Identity::Human {
        human_id: HumanId::try_from("other".to_owned()).expect("human"),
    };
    let wrong = client
        .answer_question(QuestionAnswerParams {
            request_id: "question-1".into(),
            actor: wrong_actor,
            response: answer.clone(),
        })
        .await
        .expect_err("wrong actor");
    let failure = wrong.into_parts().0;
    assert_eq!(
        failure.service_kind.as_deref(),
        Some("wrongActor"),
        "{failure:?}"
    );
    // A rejected Control exchange retires its client connection; a fresh
    // front door can still answer the same pending question.
    drop(client);
    server
        .await
        .expect("first server task")
        .expect("first server");
    let (client_stream, server_stream) =
        tokio::net::UnixStream::pair().expect("second socket pair");
    let server = tokio::spawn(serve_control_connection(server_stream, identity));
    let mut client = ControlClient::initialize(client_stream, "question-test", "1")
        .await
        .expect("reinitialize");
    let receipt = client
        .answer_question(QuestionAnswerParams {
            request_id: "question-1".into(),
            actor: approver,
            response: answer.clone(),
        })
        .await
        .expect("answer");
    assert_eq!(receipt.state, QuestionState::Answered);
    assert_eq!(agent_reply.await.expect("agent reply"), answer);
    let mut observer = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.path().join("interaction.sqlite"))
            .read_only(true),
    )
    .await
    .expect("independent durable observation");
    let record_json: String = sqlx::query_scalar(
        "SELECT record_json FROM interaction_history_records WHERE request_id='question-1'",
    )
    .fetch_one(&mut observer)
    .await
    .expect("actual answer persisted through Control route");
    let stored: collaboration_service::InteractionHistoryRecord =
        serde_json::from_str(&record_json).expect("typed stored answer");
    let collaboration_service::InteractionHistoryRecord::Question {
        state: collaboration_service::QuestionHistoryState::Answered { content },
        ..
    } = stored
    else {
        panic!("durable Question must be answered");
    };
    assert_eq!(QuestionResponse::Answered { content }, answer);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM interaction_history_records")
        .fetch_one(&mut observer)
        .await
        .expect("import plus real route mutation");
    assert_eq!(count, 2);
    assert_eq!(
        tokio::fs::read(root.path().join("interaction-history.json"))
            .await
            .expect("recovery bytes"),
        original
    );
    assert!(
        client
            .list_questions(true)
            .await
            .expect("pending list")
            .questions
            .is_empty()
    );
    client.close().await.expect("close");
    server.await.expect("server task").expect("server");
}
