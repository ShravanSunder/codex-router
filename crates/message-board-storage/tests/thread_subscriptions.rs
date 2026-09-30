#![allow(clippy::unwrap_used)]
#[path = "thread_subscriptions/lifecycle.rs"]
mod lifecycle;
#[path = "thread_subscriptions/migration.rs"]
mod migration;
#[path = "thread_subscriptions/review_minor_regressions.rs"]
mod review_minor_regressions;
#[path = "thread_subscriptions/review_regressions.rs"]
mod review_regressions;
#[path = "thread_subscriptions/windows.rs"]
mod windows;

use chrono::{DateTime, Utc};
use message_board::*;
use message_board_storage::BoardStore;

fn persisted_time(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(value.timestamp_millis()).unwrap()
}

struct ThreadSubscriptionFixture {
    path: std::path::PathBuf,
    store: BoardStore,
    root_message_id: MessageId,
    topic_id: TopicId,
    project_id: ProjectId,
    reader: Identity,
    now: DateTime<Utc>,
}

impl ThreadSubscriptionFixture {
    async fn create(label: &str) -> Self {
        let mut fixture = Self::create_without_participant(label).await;
        let reader = fixture.reader.clone();
        let now = fixture.now;
        fixture
            .join_reader(reader, ParticipantRole::Participant, true, now)
            .await;
        fixture
    }

    async fn create_without_participant(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "thread-subscriptions-{label}-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let mut store = BoardStore::open(&path).await.unwrap();
        let now = Utc::now();
        let project_id = ProjectId::generate();
        let board_id = BoardId::generate();
        let topic_id = TopicId::generate();
        store
            .create_project(ProjectCreateRequest {
                project_id: project_id.clone(),
                name: name("Subscriptions project"),
                description: description("Storage proof"),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_board(BoardCreateRequest {
                board_id: board_id.clone(),
                project_id: project_id.clone(),
                name: name("Subscriptions board"),
                description: description("Storage proof"),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        store
            .create_topic(TopicCreateRequest {
                topic_id: topic_id.clone(),
                board_id,
                name: name("Subscriptions topic"),
                description: description("Storage proof"),
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        let root = post(
            &mut store,
            Placement::Topic {
                topic_id: topic_id.clone(),
            },
            human("owner"),
            "Root",
            now,
        )
        .await;
        let reader = session("reader");
        Self {
            path,
            store,
            root_message_id: root.message.message_id,
            topic_id,
            project_id,
            reader,
            now,
        }
    }

    async fn post_reply(&mut self, author: &str, body: &str) -> MessagePostResult {
        self.post_reply_as(human(author), body, self.now).await
    }

    async fn post_reply_as(
        &mut self,
        actor: Identity,
        body: &str,
        now: DateTime<Utc>,
    ) -> MessagePostResult {
        post(
            &mut self.store,
            Placement::Thread {
                root_message_id: self.root_message_id.clone(),
            },
            actor,
            body,
            now,
        )
        .await
    }

    async fn post(
        &mut self,
        placement: Placement,
        actor: Identity,
        body: &str,
        now: DateTime<Utc>,
    ) -> MessagePostResult {
        post(&mut self.store, placement, actor, body, now).await
    }

    async fn create_root(&mut self, author: Identity, body: &str, now: DateTime<Utc>) -> MessageId {
        post(
            &mut self.store,
            Placement::Topic {
                topic_id: self.topic_id.clone(),
            },
            author,
            body,
            now,
        )
        .await
        .message
        .message_id
    }

    async fn join_reader(
        &mut self,
        reader: Identity,
        role: ParticipantRole,
        watch: bool,
        now: DateTime<Utc>,
    ) -> ThreadJoinResult {
        self.store
            .join_thread(
                ThreadJoinRequest {
                    root_message_id: self.root_message_id.clone(),
                    actor: reader,
                    role,
                    watch,
                    replace: None,
                    note: None,
                },
                now,
            )
            .await
            .unwrap()
    }

    async fn watch_status(&mut self, reader: Identity) -> WatchStatus {
        self.watch_status_for(self.root_message_id.clone(), reader)
            .await
    }

    async fn watch_status_for(
        &mut self,
        root_message_id: MessageId,
        reader: Identity,
    ) -> WatchStatus {
        self.store
            .show_thread(ThreadShowRequest {
                root_message_id,
                reader: Some(reader),
            })
            .await
            .unwrap()
            .watch_status
            .unwrap()
    }

    fn listen_request(&self) -> ThreadListenRequest {
        ThreadListenRequest {
            reader: self.reader.clone(),
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![self.root_message_id.clone()],
            },
            mode: ThreadListenMode::Repeating {
                lifetime_seconds: 60 * 60,
            },
            from_activity_sequence: None,
            acknowledge: false,
            delivery: ThreadListenDelivery::Session,
        }
    }

    async fn finish(self) {
        self.store.close().await.unwrap();
        std::fs::remove_file(self.path).unwrap();
    }
}

#[tokio::test]
async fn delivered_position_never_moves_backwards_during_late_settlement() {
    let mut fixture = ThreadSubscriptionFixture::create("monotonic-delivered").await;
    let first = fixture.post_reply("first-author", "first reply").await;
    let second = fixture.post_reply("second-author", "second reply").await;
    let context = fixture
        .store
        .prepare_thread_listen(&fixture.listen_request())
        .await
        .unwrap();
    let batch_set = fixture
        .store
        .select_pending_thread_listen_batch_set(ListenId::generate(), &context, 32_000)
        .await
        .unwrap();
    assert_eq!(
        batch_set.batches[0].delivered_through,
        second.message.activity_sequence
    );

    fixture
        .store
        .record_thread_listen_batch_delivery(&context, &batch_set)
        .await
        .unwrap();

    let mut older_settlement = batch_set;
    older_settlement.batches[0].delivered_through = first.message.activity_sequence;
    fixture
        .store
        .record_thread_listen_batch_delivery(&context, &older_settlement)
        .await
        .unwrap();

    let pending_after_late_settlement = fixture
        .store
        .select_pending_thread_listen_batch_set(ListenId::generate(), &context, 32_000)
        .await
        .unwrap();
    assert!(
        pending_after_late_settlement.batches.is_empty(),
        "a late lower through-position must not make the newer reply selectable again"
    );

    fixture.finish().await;
}

async fn post(
    store: &mut BoardStore,
    placement: Placement,
    actor: Identity,
    body: &str,
    now: DateTime<Utc>,
) -> MessagePostResult {
    store
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement,
                actor,
                acting_for: None,
                text: text(body),
                references: no_references(),
            },
            now,
        )
        .await
        .unwrap()
}

fn session(value: &str) -> Identity {
    Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from("550e8400-e29b-41d4-a716-446655440000".to_owned())
                    .unwrap(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).unwrap(),
            },
            session_id: SessionId::try_from(value.to_owned()).unwrap(),
        },
    }
}

fn human(value: &str) -> Identity {
    Identity::Human {
        human_id: HumanId::try_from(value.to_owned()).unwrap(),
    }
}

fn name(value: &str) -> ResourceName {
    ResourceName::try_from(value.to_owned()).unwrap()
}

fn description(value: &str) -> Description {
    Description::try_from(value.to_owned()).unwrap()
}

fn text(value: &str) -> MessageText {
    MessageText::try_from(value.to_owned()).unwrap()
}

fn no_references() -> MessageReferences {
    MessageReferences::try_from(Vec::new()).unwrap()
}
