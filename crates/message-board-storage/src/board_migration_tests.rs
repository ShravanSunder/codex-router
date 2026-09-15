#![allow(clippy::unwrap_used)]
//! Real file-backed SQLx migration proofs over populated board-domain state.
use super::*;
use crate::BoardStore;
use sqlx::SqlSafeStr;
use sqlx::migrate::{Migration, MigrationType, Migrator};
use std::borrow::Cow;

#[path = "board_migration_test_support.rs"]
mod support;
use support::*;

const ADDITIVE: &str = "ALTER TABLE board_projects ADD COLUMN migration_note TEXT; CREATE INDEX board_projects_migration_note ON board_projects(migration_note);";
const REBUILD: &str = "CREATE TABLE replacement_projects(project_id TEXT PRIMARY KEY NOT NULL,name TEXT NOT NULL,description TEXT NOT NULL) STRICT; INSERT INTO replacement_projects(project_id,name,description) SELECT project_id,name,description FROM board_projects; DROP TABLE board_projects; ALTER TABLE replacement_projects RENAME TO board_projects; CREATE UNIQUE INDEX board_projects_name_unique ON board_projects(name);";

fn current_schema() -> String {
    format!("{BASELINE} {THREAD_DELIVERY_POSITIONS}")
}

fn migrator(version: i64, description: &'static str, extra: &str) -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![
            Migration::new(
                202609120001,
                "project board".into(),
                MigrationType::Simple,
                BASELINE.into_sql_str(),
                false,
            ),
            Migration::new(
                202609140001,
                "thread delivery positions".into(),
                MigrationType::Simple,
                THREAD_DELIVERY_POSITIONS.into_sql_str(),
                false,
            ),
            Migration::new(
                version,
                description.into(),
                MigrationType::Simple,
                sqlx::AssertSqlSafe(extra.to_owned()).into_sql_str(),
                false,
            ),
        ]),
        ..Migrator::DEFAULT
    }
}

fn baseline_migrator() -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![Migration::new(
            202609120001,
            "project board".into(),
            MigrationType::Simple,
            BASELINE.into_sql_str(),
            false,
        )]),
        ..Migrator::DEFAULT
    }
}

#[tokio::test]
async fn delivered_position_migration_preserves_populated_thread_and_watch_state() {
    let path = std::path::PathBuf::from("/tmp").join(format!(
        "board-thread-delivery-migration-{}.sqlite",
        message_board::MessageId::generate().as_str()
    ));
    let mut connection =
        SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
    initialize_with(&mut connection, &baseline_migrator(), BASELINE)
        .await
        .unwrap();
    let owner = migration_actor("owner");
    let reader = migration_actor("reader");
    let project_id = message_board::ProjectId::generate();
    let board_id = message_board::BoardId::generate();
    let topic_id = message_board::TopicId::generate();
    let root_message_id = message_board::MessageId::generate();
    let owner_key = crate::storage_support::identity_key(&owner);
    let reader_key = crate::storage_support::identity_key(&reader);
    sqlx::query("INSERT INTO board_identities(identity_key,kind,human_id) VALUES(?,'human','owner'),(?,'human','reader')")
        .bind(&owner_key)
        .bind(&reader_key)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO board_projects(project_id,name,description) VALUES(?,'Project','')")
        .bind(project_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_boards(board_id,project_id,name,description,state) VALUES(?,?,'Board','','active')")
        .bind(board_id.as_str())
        .bind(project_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO board_topics(topic_id,board_id,name,description) VALUES(?,?,'Topic','')",
    )
    .bind(topic_id.as_str())
    .bind(board_id.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query("INSERT INTO board_messages(message_id,topic_id,board_id,root_id,actor_key,text) VALUES(?,?,?,NULL,?,'Root')")
        .bind(root_message_id.as_str())
        .bind(topic_id.as_str())
        .bind(board_id.as_str())
        .bind(&owner_key)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO board_threads(root_id,state) VALUES(?,'unresolved')")
        .bind(root_message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("UPDATE activity_checkpoint SET last_sequence=1 WHERE singleton=1")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) VALUES(1,?,?,?,NULL,'mainMessageCreated',?,?)")
        .bind(project_id.as_str())
        .bind(board_id.as_str())
        .bind(topic_id.as_str())
        .bind(&owner_key)
        .bind(root_message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reader_state(reader_key,project_id,main_start,has_unread) VALUES(?,?,NULL,0)")
        .bind(&reader_key)
        .bind(project_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO thread_watches(reader_key,root_id,active,starts_after_activity) VALUES(?,?,1,1)")
        .bind(&reader_key)
        .bind(root_message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    let mut store = store_from_connection(connection).await;
    let before = store
        .show_thread(message_board::ThreadShowRequest {
            root_message_id: root_message_id.clone(),
            reader: Some(reader.clone()),
        })
        .await
        .unwrap();
    store.close().await.unwrap();

    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    initialize_with(&mut connection, &MIGRATOR, &current_schema())
        .await
        .unwrap();
    let mut store = store_from_connection(connection).await;
    assert_eq!(
        store
            .show_thread(message_board::ThreadShowRequest {
                root_message_id,
                reader: Some(reader),
            })
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        migration_versions(&mut store.connection).await,
        vec![202609120001, 202609140001]
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='thread_delivery_positions')"
        )
        .fetch_one(&mut store.connection)
        .await
        .unwrap()
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

fn migration_actor(value: &str) -> message_board::Identity {
    message_board::Identity::Human {
        human_id: value.to_owned().try_into().unwrap(),
    }
}

#[tokio::test]
async fn additive_migration_preserves_semantic_state_and_history() {
    let mut fixture = PopulatedBoard::create("additive").await;
    let before = fixture.snapshot().await;
    let mut connection = fixture.close_and_connect_for_migration().await;
    let expected = format!("{} {ADDITIVE}", current_schema());
    initialize_with(
        &mut connection,
        &migrator(202609150001, "test-only additive", ADDITIVE),
        &expected,
    )
    .await
    .unwrap();
    let mut store = store_from_connection(connection).await;
    assert_eq!(fixture.snapshot_with(&mut store).await, before);
    assert_eq!(
        migration_versions(&mut store.connection).await,
        vec![202609120001, 202609140001, 202609150001]
    );
    assert!(index_exists(&mut store.connection, "board_projects_migration_note").await);
    assert!(
        foreign_key_violations(&mut store.connection)
            .await
            .is_empty()
    );
    fixture.finish(store).await;
}

#[tokio::test]
async fn populated_parent_rebuild_preserves_exact_rows_domain_reads_and_relationships() {
    let mut fixture = PopulatedBoard::create("rebuild").await;
    let before = fixture.snapshot().await;
    let before_rows = fixture.raw_rows().await;
    let mut connection = fixture.close_and_connect_for_migration().await;
    let expected = format!("{} {REBUILD}", current_schema());
    initialize_with(
        &mut connection,
        &migrator(202609150001, "test-only rebuild", REBUILD),
        &expected,
    )
    .await
    .unwrap();
    let mut store = store_from_connection(connection).await;
    assert_eq!(fixture.snapshot_with(&mut store).await, before);
    assert_eq!(all_domain_rows(&mut store.connection).await, before_rows);
    assert_eq!(
        migration_versions(&mut store.connection).await,
        vec![202609120001, 202609140001, 202609150001]
    );
    for index in [
        "board_projects_name_unique",
        "project_boards_key_2",
        "board_topics_key_2",
        "board_messages_key_1",
    ] {
        assert!(
            index_exists(&mut store.connection, index).await,
            "missing {index}"
        );
    }
    assert!(
        foreign_key_violations(&mut store.connection)
            .await
            .is_empty()
    );
    fixture.finish(store).await;
}

#[tokio::test]
async fn failed_rebuild_reopens_separately_with_original_exact_state_and_history() {
    let mut fixture = PopulatedBoard::create("failed-rebuild").await;
    let before = fixture.snapshot().await;
    let before_rows = fixture.raw_rows().await;
    let before_schema = fixture.schema_objects().await;
    let mut connection = fixture.close_and_connect_for_migration().await;
    let failing = format!("{REBUILD} INSERT INTO missing_table VALUES(1);");
    assert!(
        initialize_with(
            &mut connection,
            &migrator(202609150001, "test-only failing rebuild", &failing),
            &current_schema()
        )
        .await
        .is_err()
    );
    connection.close().await.unwrap();
    let mut reopened = BoardStore::open(&fixture.path).await.unwrap();
    assert_eq!(fixture.snapshot_with(&mut reopened).await, before);
    assert_eq!(all_domain_rows(&mut reopened.connection).await, before_rows);
    assert_eq!(
        definitions(&mut reopened.connection).await.unwrap(),
        before_schema
    );
    assert_eq!(
        migration_versions(&mut reopened.connection).await,
        vec![202609120001, 202609140001]
    );
    fixture.finish(reopened).await;
}

#[tokio::test]
async fn broken_relationship_rejects_migration_and_reopens_unchanged() {
    let mut fixture = PopulatedBoard::create("invalid-relationship").await;
    let before_rows = fixture.raw_rows().await;
    let invalid = format!(
        "UPDATE project_boards SET project_id='{}' WHERE board_id='{}';",
        message_board::ProjectId::generate().as_str(),
        fixture.first_board_id.as_str()
    );
    let mut connection = fixture.close_and_connect_for_migration().await;
    assert!(
        initialize_with(
            &mut connection,
            &migrator(202609150001, "test-only invalid relationship", &invalid),
            &current_schema()
        )
        .await
        .is_err()
    );
    connection.close().await.unwrap();
    let mut reopened = BoardStore::open(&fixture.path).await.unwrap();
    assert_eq!(all_domain_rows(&mut reopened.connection).await, before_rows);
    assert_eq!(
        migration_versions(&mut reopened.connection).await,
        vec![202609120001, 202609140001]
    );
    assert!(
        foreign_key_violations(&mut reopened.connection)
            .await
            .is_empty()
    );
    fixture.finish(reopened).await;
}

#[tokio::test]
async fn foreign_key_enablement_rejects_an_active_transaction() {
    let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
    assert!(matches!(
        enable_foreign_keys(&mut connection).await,
        Err(BoardStorageError::InvalidSchema)
    ));
    sqlx::query("ROLLBACK")
        .execute(&mut connection)
        .await
        .unwrap();
}
