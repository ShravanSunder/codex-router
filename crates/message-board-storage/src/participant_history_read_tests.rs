#![allow(clippy::unwrap_used)]
use crate::participant_history_test_support::*;
use message_board::{ParticipantRole::*, *};

async fn assert_roles(fixture: &mut HistoryFixture, expected: &[(MessageId, &str)]) {
    for (message_id, role) in expected {
        let shown = fixture
            .store
            .show_message(MessageShowRequest {
                message_id: message_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(shown.0).unwrap()["postedAsRole"],
            *role
        );
    }
}

#[tokio::test]
async fn participant_history_reads_roles_on_every_message_path_and_keeps_lifetime() {
    let mut fixture = HistoryFixture::open().await;
    let created = fixture.create(session("O"), Some(Orchestrator)).await;
    let root = created.message_id.clone();
    assert_eq!(
        serde_json::to_value(&created).unwrap()["postedAsRole"],
        "orchestrator"
    );
    let mut expected = vec![(root.clone(), "orchestrator")];
    fixture.join(&root, session("A"), Reviewer, None).await;
    let reply = fixture.post(&root, session("A")).await;
    assert_eq!(
        serde_json::to_value(&reply).unwrap()["postedAsRole"],
        "reviewer"
    );
    expected.push((reply.message_id, "reviewer"));
    fixture
        .leave(&root, session("O"), Some(session("A")), false)
        .await;
    assert_roles(&mut fixture, &expected).await;
    let reply = fixture.post(&root, session("A")).await;
    expected.push((reply.message_id, "orchestrator"));
    fixture
        .join(&root, session("O"), Orchestrator, Some(session("A")))
        .await;
    assert_roles(&mut fixture, &expected).await;
    fixture.join(&root, session("A"), Advisor, None).await;
    assert_roles(&mut fixture, &expected).await;
    expected.push((
        fixture.post(&root, session("A")).await.message_id,
        "advisor",
    ));
    fixture.join(&root, session("B"), Implementer, None).await;
    fixture
        .join(&root, session("A"), Implementer, Some(session("B")))
        .await;
    assert_roles(&mut fixture, &expected).await;
    expected.push((
        fixture.post(&root, session("A")).await.message_id,
        "implementer",
    ));
    fixture.leave(&root, session("O"), None, true).await;
    assert_roles(&mut fixture, &expected).await;
    fixture.unresolve(&root).await;
    assert_roles(&mut fixture, &expected).await;
    fixture.join(&root, session("A"), Participant, None).await;
    assert_roles(&mut fixture, &expected).await;
    expected.push((
        fixture.post(&root, session("A")).await.message_id,
        "participant",
    ));
    assert_roles(&mut fixture, &expected).await;
    let listed = fixture
        .store
        .list_messages(MessageListRequest {
            scope: MessageListScope::Thread {
                root_message_id: root,
            },
            selection: MessageSelection::Latest,
            page: PageRequest {
                limit: PageLimit::try_from(100).unwrap(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    for message in listed.page.records {
        let role = expected
            .iter()
            .find(|(id, _)| *id == message.message_id)
            .unwrap()
            .1;
        assert_eq!(serde_json::to_value(message).unwrap()["postedAsRole"], role);
    }
}

#[tokio::test]
async fn participant_history_reads_omit_unknown_and_human_roles() {
    let mut fixture = HistoryFixture::open().await;
    let human_root = fixture.create(human("H"), Some(Reviewer)).await;
    let human_reply = fixture.post(&human_root.message_id, human("H")).await;
    let root = fixture.create(session("O"), Some(Orchestrator)).await;
    let reply = fixture.post(&root.message_id, session("O")).await;
    sqlx::query("UPDATE board_messages SET posted_from_activity=NULL WHERE message_id=?")
        .bind(reply.message_id.as_str())
        .execute(&mut fixture.store.connection)
        .await
        .unwrap();
    for message in [&human_root, &human_reply, &reply] {
        let shown = fixture
            .store
            .show_message(MessageShowRequest {
                message_id: message.message_id.clone(),
            })
            .await
            .unwrap();
        assert!(
            serde_json::to_value(shown.0)
                .unwrap()
                .get("postedAsRole")
                .is_none()
        );
    }
    // Older Host payloads remain decodable by the new defaulted wire field.
    let old_json = serde_json::to_value(human_reply).unwrap();
    serde_json::from_value::<Message>(old_json).unwrap();
}

#[tokio::test]
async fn participant_history_reads_fail_closed_for_corrupt_attribution_relationships() {
    for corruption in [
        "invalidRole",
        "wrongAuthor",
        "wrongRoot",
        "laterSequence",
        "noRole",
        "mainWrongKind",
    ] {
        let mut fixture = HistoryFixture::open().await;
        let root = fixture.create(session("O"), Some(Orchestrator)).await;
        let grant = fixture
            .join(&root.message_id, session("A"), Reviewer, None)
            .await;
        let reply = fixture.post(&root.message_id, session("A")).await;
        let message = if corruption == "mainWrongKind" {
            &root
        } else {
            &reply
        };
        match corruption {
            "invalidRole" => {
                sqlx::query(
                    "UPDATE board_activity SET participant_role='bogus' WHERE activity_sequence=?",
                )
                .bind(grant)
                .execute(&mut fixture.store.connection)
                .await
                .unwrap();
            }
            "wrongAuthor" => {
                sqlx::query("UPDATE board_activity SET participant_key=actor_key WHERE activity_sequence=(SELECT posted_from_activity FROM board_messages WHERE message_id=?)").bind(root.message_id.as_str()).execute(&mut fixture.store.connection).await.unwrap();
                sqlx::query("UPDATE board_messages SET posted_from_activity=(SELECT posted_from_activity FROM board_messages WHERE message_id=?) WHERE message_id=?").bind(root.message_id.as_str()).bind(reply.message_id.as_str()).execute(&mut fixture.store.connection).await.unwrap();
            }
            "wrongRoot" => {
                let other = fixture.create(human("other"), None).await;
                sqlx::query("UPDATE board_activity SET root_id=? WHERE activity_sequence=?")
                    .bind(other.message_id.as_str())
                    .bind(grant)
                    .execute(&mut fixture.store.connection)
                    .await
                    .unwrap();
            }
            "laterSequence" => {
                let later = fixture
                    .join(&root.message_id, session("A"), Advisor, None)
                    .await;
                sqlx::query("UPDATE board_messages SET posted_from_activity=? WHERE message_id=?")
                    .bind(later)
                    .bind(reply.message_id.as_str())
                    .execute(&mut fixture.store.connection)
                    .await
                    .unwrap();
            }
            "noRole" => {
                sqlx::query(
                    "UPDATE board_activity SET participant_role=NULL WHERE activity_sequence=?",
                )
                .bind(grant)
                .execute(&mut fixture.store.connection)
                .await
                .unwrap();
            }
            "mainWrongKind" => {
                sqlx::query("UPDATE board_activity SET kind='orchestratorReplaced' WHERE activity_sequence=(SELECT posted_from_activity FROM board_messages WHERE message_id=?)").bind(root.message_id.as_str()).execute(&mut fixture.store.connection).await.unwrap();
            }
            _ => unreachable!(),
        }
        let error = fixture
            .store
            .show_message(MessageShowRequest {
                message_id: message.message_id.clone(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind, BoardFailureKind::InvalidRecord, "{corruption}");
        assert_eq!(
            error.details,
            BoardErrorDetails::Resource {
                resource: ResourceIdentity::Message {
                    message_id: message.message_id.clone()
                }
            }
        );
    }
}
