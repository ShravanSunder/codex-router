#![allow(clippy::unwrap_used)]
//! Real BoardStore fixture shared by history write/read/property proofs.
use crate::{BoardStore, storage_support::identity_key};
use message_board::*;
use sqlx::Connection;

pub(super) fn session(label: &str) -> Identity {
    Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: "550e8400-e29b-41d4-a716-446655440000"
                    .to_owned()
                    .try_into()
                    .unwrap(),
                endpoint_id: "codex-local".to_owned().try_into().unwrap(),
            },
            session_id: label.to_owned().try_into().unwrap(),
        },
    }
}
pub(super) fn human(label: &str) -> Identity {
    Identity::Human {
        human_id: label.to_owned().try_into().unwrap(),
    }
}
pub(super) struct HistoryFixture {
    pub store: BoardStore,
    pub topic: TopicId,
}
impl HistoryFixture {
    pub async fn open() -> Self {
        let mut connection = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .unwrap();
        crate::board_schema_migrations::initialize(&mut connection)
            .await
            .unwrap();
        let key: Vec<u8> = sqlx::query_scalar("SELECT cursor_key FROM activity_checkpoint")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        let mut store = BoardStore {
            connection,
            cursor_key: key.try_into().unwrap(),
            activity_sender: crate::board_connection::activity_sender(),
        };
        let project = ProjectId::generate();
        let board = BoardId::generate();
        let topic = TopicId::generate();
        store
            .create_project(ProjectCreateRequest {
                project_id: project.clone(),
                name: "Project".to_owned().try_into().unwrap(),
                description: "".to_owned().try_into().unwrap(),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_board(BoardCreateRequest {
                project_id: project,
                board_id: board.clone(),
                name: "Board".to_owned().try_into().unwrap(),
                description: "".to_owned().try_into().unwrap(),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_topic(TopicCreateRequest {
                board_id: board,
                topic_id: topic.clone(),
                name: "Topic".to_owned().try_into().unwrap(),
                description: "".to_owned().try_into().unwrap(),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        Self { store, topic }
    }
    pub async fn create(&mut self, actor: Identity, role: Option<ParticipantRole>) -> Message {
        self.store
            .create_thread(
                ThreadCreateRequest {
                    message_id: MessageId::generate(),
                    topic_id: self.topic.clone(),
                    actor,
                    role,
                    watch: false,
                    acting_for: None,
                    text: "Root".to_owned().try_into().unwrap(),
                    references: Vec::new().try_into().unwrap(),
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap()
            .message
    }
    pub async fn join(
        &mut self,
        root: &MessageId,
        actor: Identity,
        role: ParticipantRole,
        replace: Option<Identity>,
    ) -> i64 {
        self.store
            .join_thread(
                ThreadJoinRequest {
                    root_message_id: root.clone(),
                    actor,
                    role,
                    replace,
                    watch: false,
                    note: None,
                    mode: None,
                    when_idle: None,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap()
            .participant
            .joined_at_activity
            .get()
            .try_into()
            .unwrap()
    }
    pub async fn leave(
        &mut self,
        root: &MessageId,
        actor: Identity,
        to: Option<Identity>,
        resolve: bool,
    ) -> i64 {
        self.store
            .leave_thread(
                ThreadLeaveRequest {
                    root_message_id: root.clone(),
                    actor,
                    to,
                    resolve,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        self.latest().await
    }
    pub async fn latest(&mut self) -> i64 {
        sqlx::query_scalar("SELECT last_sequence FROM activity_checkpoint")
            .fetch_one(&mut self.store.connection)
            .await
            .unwrap()
    }
    pub async fn resolve(&mut self, root: &MessageId, actor: Identity) -> i64 {
        self.store
            .resolve_thread(
                ThreadResolveRequest {
                    root_message_id: root.clone(),
                    actor,
                    acting_for: None,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        self.latest().await
    }
    pub async fn unresolve(&mut self, root: &MessageId) -> i64 {
        self.store
            .unresolve_thread(ThreadUnresolveRequest {
                root_message_id: root.clone(),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        self.latest().await
    }
    pub async fn post(&mut self, root: &MessageId, actor: Identity) -> Message {
        post(&mut self.store, root, actor).await
    }
}
pub(super) async fn post(store: &mut BoardStore, root: &MessageId, actor: Identity) -> Message {
    store
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root.clone(),
                },
                actor,
                acting_for: None,
                text: "Reply".to_owned().try_into().unwrap(),
                references: Vec::new().try_into().unwrap(),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap()
        .message
}
pub(super) async fn assert_change(
    store: &mut BoardStore,
    sequence: i64,
    participant: Option<Identity>,
    role: Option<ParticipantRole>,
    replaced: Option<Identity>,
) {
    let row: (Option<String>, Option<String>, Option<String>) = sqlx::query_as("SELECT participant_key,participant_role,replaced_participant_key FROM board_activity WHERE activity_sequence=?").bind(sequence).fetch_one(&mut store.connection).await.unwrap();
    assert_eq!(
        row,
        (
            participant.as_ref().map(identity_key),
            role.map(crate::participant_row_decoding::role_name)
                .map(str::to_owned),
            replaced.as_ref().map(identity_key)
        ),
        "activity {sequence}"
    );
}
pub(super) async fn posted_from(store: &mut BoardStore, message: &MessageId) -> Option<i64> {
    sqlx::query_scalar("SELECT posted_from_activity FROM board_messages WHERE message_id=?")
        .bind(message.as_str())
        .fetch_one(&mut store.connection)
        .await
        .unwrap()
}
