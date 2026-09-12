use crate::BoardStore;
use project_board::*;
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::path::PathBuf;

pub(super) struct PopulatedBoard {
    pub(super) path: PathBuf,
    store: Option<BoardStore>,
    first_project_id: ProjectId,
    second_project_id: ProjectId,
    pub(super) first_board_id: BoardId,
    first_root_id: MessageId,
    second_root_id: MessageId,
    reader: Identity,
}

impl PopulatedBoard {
    pub(super) async fn create(label: &str) -> Self {
        let path = PathBuf::from("/tmp").join(format!(
            "board-migration-{label}-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let mut store = BoardStore::open(&path).await.unwrap();
        let first_project_id = ProjectId::generate();
        let second_project_id = ProjectId::generate();
        let first_board_id = BoardId::generate();
        let second_board_id = BoardId::generate();
        let first_topic_id = TopicId::generate();
        let second_topic_id = TopicId::generate();
        create_metadata(
            &mut store,
            &first_project_id,
            &first_board_id,
            &first_topic_id,
            "First",
        )
        .await;
        create_metadata(
            &mut store,
            &second_project_id,
            &second_board_id,
            &second_topic_id,
            "Second",
        )
        .await;
        store
            .attach_repository(RepositoryAttachRequest {
                project_id: first_project_id.clone(),
                repository: RepositoryRef::Origin {
                    normalized_origin: NormalizedOrigin::try_from(
                        "github.com/example/first".to_owned(),
                    )
                    .unwrap(),
                },
                actor: human("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
        let reader = human("reader");
        for project_id in [&first_project_id, &second_project_id] {
            store
                .fetch_inbox(InboxFetchRequest {
                    project_id: project_id.clone(),
                    reader: reader.clone(),
                    page: page(),
                })
                .await
                .unwrap();
        }
        let first_root = post(
            &mut store,
            Placement::Topic {
                topic_id: first_topic_id,
            },
            human("first-author"),
            "first root",
            vec![],
        )
        .await;
        let second_root = post(
            &mut store,
            Placement::Topic {
                topic_id: second_topic_id,
            },
            human("second-author"),
            "second root",
            vec![],
        )
        .await;
        store
            .watch_thread(ThreadWatchRequest {
                root_message_id: first_root.message.message_id.clone(),
                actor: reader.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
        let reply = post(
            &mut store,
            Placement::Thread {
                root_message_id: first_root.message.message_id.clone(),
            },
            human("reply-author"),
            "cross-project reply",
            vec![
                ReferenceTarget::Message {
                    message_id: second_root.message.message_id.clone(),
                },
                ReferenceTarget::Thread {
                    root_message_id: second_root.message.message_id.clone(),
                },
            ],
        )
        .await;
        store
            .acknowledge_inbox(InboxAcknowledgeRequest {
                actor: reader.clone(),
                acting_for: None,
                scope: ReadScope::Thread {
                    root_message_id: first_root.message.message_id.clone(),
                },
                through_activity_sequence: reply.message.activity_sequence,
            })
            .await
            .unwrap();
        store
            .resolve_thread(ThreadResolveRequest {
                root_message_id: first_root.message.message_id.clone(),
                actor: human("first-author"),
                acting_for: None,
            })
            .await
            .unwrap();
        Self {
            path,
            store: Some(store),
            first_project_id,
            second_project_id,
            first_board_id,
            first_root_id: first_root.message.message_id,
            second_root_id: second_root.message.message_id,
            reader,
        }
    }

    pub(super) async fn snapshot(&mut self) -> Value {
        let mut store = self.store.take().unwrap();
        let snapshot = self.snapshot_with(&mut store).await;
        self.store = Some(store);
        snapshot
    }

    pub(super) async fn snapshot_with(&self, store: &mut BoardStore) -> Value {
        json!({
            "firstProject": store.show_project(ProjectShowRequest { project_id: self.first_project_id.clone() }).await.unwrap(),
            "secondProject": store.show_project(ProjectShowRequest { project_id: self.second_project_id.clone() }).await.unwrap(),
            "repositories": store.list_repositories(RepositoryListRequest { project_id: self.first_project_id.clone(), page: page() }).await.unwrap(),
            "root": store.show_message(MessageShowRequest { message_id: self.first_root_id.clone() }).await.unwrap(),
            "crossProjectTarget": store.show_message(MessageShowRequest { message_id: self.second_root_id.clone() }).await.unwrap(),
            "thread": store.show_thread(ThreadShowRequest { root_message_id: self.first_root_id.clone(), reader: Some(self.reader.clone()) }).await.unwrap(),
            "history": store.list_messages(MessageListRequest { scope: MessageListScope::Thread { root_message_id: self.first_root_id.clone() }, selection: MessageSelection::Latest, page: page() }).await.unwrap(),
            "inbox": store.fetch_inbox(InboxFetchRequest { project_id: self.first_project_id.clone(), reader: self.reader.clone(), page: page() }).await.unwrap(),
            "summaries": store.list_inbox_projects(InboxProjectsRequest { reader: self.reader.clone(), unread_only: false, page: page() }).await.unwrap(),
        })
    }

    pub(super) async fn raw_rows(&mut self) -> Vec<(String, Vec<String>)> {
        all_domain_rows(&mut self.store.as_mut().unwrap().connection).await
    }
    pub(super) async fn schema_objects(&mut self) -> Vec<(String, String)> {
        super::definitions(&mut self.store.as_mut().unwrap().connection)
            .await
            .unwrap()
    }
    pub(super) async fn close_and_connect_for_migration(&mut self) -> SqliteConnection {
        self.store.take().unwrap().close().await.unwrap();
        let mut connection =
            SqliteConnection::connect(&format!("sqlite://{}", self.path.display()))
                .await
                .unwrap();
        sqlx::query("PRAGMA foreign_keys=OFF")
            .execute(&mut connection)
            .await
            .unwrap();
        connection
    }
    pub(super) async fn finish(mut self, store: BoardStore) {
        store.close().await.unwrap();
        self.store = None;
        std::fs::remove_file(&self.path).unwrap();
    }
}

impl Drop for PopulatedBoard {
    fn drop(&mut self) {
        if self.store.is_none() {
            let _cleanup = std::fs::remove_file(&self.path);
        }
    }
}

async fn create_metadata(
    store: &mut BoardStore,
    project_id: &ProjectId,
    board_id: &BoardId,
    topic_id: &TopicId,
    suffix: &str,
) {
    store
        .create_project(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: name(&format!("{suffix} project")),
            description: description("migration fixture"),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_board(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id: project_id.clone(),
            name: name(&format!("{suffix} board")),
            description: description("migration fixture"),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_topic(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id: board_id.clone(),
            name: name(&format!("{suffix} topic")),
            description: description("migration fixture"),
            actor: human("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
}
async fn post(
    store: &mut BoardStore,
    placement: Placement,
    actor: Identity,
    body: &str,
    references: Vec<ReferenceTarget>,
) -> MessagePostResult {
    store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement,
            actor,
            acting_for: None,
            text: MessageText::try_from(body.to_owned()).unwrap(),
            references: MessageReferences::try_from(references).unwrap(),
        })
        .await
        .unwrap()
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
fn page() -> PageRequest {
    PageRequest {
        limit: PageLimit::try_from(100).unwrap(),
        cursor: None,
    }
}

pub(super) async fn store_from_connection(mut connection: SqliteConnection) -> BoardStore {
    let key: Vec<u8> =
        sqlx::query_scalar("SELECT cursor_key FROM activity_checkpoint WHERE singleton=1")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    BoardStore {
        connection,
        cursor_key: key.try_into().unwrap(),
    }
}
pub(super) async fn migration_versions(connection: &mut SqliteConnection) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success=1 ORDER BY version")
        .fetch_all(connection)
        .await
        .unwrap()
}
pub(super) async fn foreign_key_violations(connection: &mut SqliteConnection) -> Vec<String> {
    sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(connection)
        .await
        .unwrap()
        .into_iter()
        .map(|row| format!("{}:{}", row.get::<String, _>(0), row.get::<i64, _>(1)))
        .collect()
}
pub(super) async fn index_exists(connection: &mut SqliteConnection, name: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?)",
    )
    .bind(name)
    .fetch_one(connection)
    .await
    .unwrap()
}
pub(super) async fn all_domain_rows(
    connection: &mut SqliteConnection,
) -> Vec<(String, Vec<String>)> {
    let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name!='_sqlx_migrations' ORDER BY name").fetch_all(&mut *connection).await.unwrap();
    let mut snapshot = Vec::new();
    for table in tables {
        let columns = sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA table_info(\"{table}\")"
        )))
        .fetch_all(&mut *connection)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
        let quoted = columns
            .iter()
            .map(|column| format!("quote(\"{column}\")"))
            .collect::<Vec<_>>()
            .join("||'|'||");
        let ordering = columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(",");
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {quoted} AS row_value FROM \"{table}\" ORDER BY {ordering}"
        )))
        .fetch_all(&mut *connection)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("row_value"))
        .collect();
        snapshot.push((table, rows));
    }
    snapshot
}
