//! Populated legacy-schema upgrade proofs.
use super::*;

fn pre_participants_migrator() -> Migrator {
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
        ]),
        ..Migrator::DEFAULT
    }
}

fn pre_topic_watch_boundary_migrator() -> Migrator {
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
                202609150001,
                "thread participants".into(),
                MigrationType::Simple,
                THREAD_PARTICIPANTS.into_sql_str(),
                false,
            ),
            Migration::new(
                202609160001,
                "thread implementer".into(),
                MigrationType::Simple,
                THREAD_IMPLEMENTER.into_sql_str(),
                false,
            ),
            Migration::new(
                202609160002,
                "topic watches".into(),
                MigrationType::Simple,
                TOPIC_WATCHES.into_sql_str(),
                false,
            ),
            Migration::new(
                202609170001,
                "thread subscriptions".into(),
                MigrationType::Simple,
                THREAD_SUBSCRIPTIONS.into_sql_str(),
                false,
            ),
        ]),
        ..Migrator::DEFAULT
    }
}

fn pre_topic_watch_boundary_schema() -> String {
    format!(
        "{BASELINE} {THREAD_DELIVERY_POSITIONS} {THREAD_PARTICIPANTS} {THREAD_IMPLEMENTER} {TOPIC_WATCHES} {THREAD_SUBSCRIPTIONS}"
    )
}

async fn topic_watch_row_bytes(
    connection: &mut SqliteConnection,
) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT hex(CAST(reader_key AS BLOB)),hex(CAST(topic_id AS BLOB)), \
           quote(starts_after_activity),quote(active) \
         FROM topic_watches ORDER BY reader_key,topic_id",
    )
    .fetch_all(connection)
    .await
    .unwrap()
}

#[tokio::test]
async fn topic_watch_boundary_migration_preserves_rows_on_a_copied_pre_change_database() {
    let suffix = message_board::MessageId::generate().as_str().to_owned();
    let original_path =
        std::env::temp_dir().join(format!("board-topic-watch-before-{suffix}.sqlite"));
    let copied_path = std::env::temp_dir().join(format!("board-topic-watch-copy-{suffix}.sqlite"));
    let mut original =
        SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", original_path.display()))
            .await
            .unwrap();
    initialize_with(
        &mut original,
        &pre_topic_watch_boundary_migrator(),
        &pre_topic_watch_boundary_schema(),
    )
    .await
    .unwrap();

    let project_id = message_board::ProjectId::generate();
    let board_id = message_board::BoardId::generate();
    let topic_id = message_board::TopicId::generate();
    let root_id = message_board::MessageId::generate();
    let owner_key = "human:topic-watch-migration-owner";
    let active_reader_key = "human:topic-watch-migration-active";
    let inactive_reader_key = "human:topic-watch-migration-inactive";
    sqlx::query(
        "INSERT INTO board_identities(identity_key,kind,human_id) \
         VALUES(?,'human','topic-watch-migration-owner'), \
               (?,'human','topic-watch-migration-active'), \
               (?,'human','topic-watch-migration-inactive')",
    )
    .bind(owner_key)
    .bind(active_reader_key)
    .bind(inactive_reader_key)
    .execute(&mut original)
    .await
    .unwrap();
    sqlx::query("INSERT INTO board_projects(project_id,name,description) VALUES(?,'Topic Watch Project','')")
        .bind(project_id.as_str())
        .execute(&mut original)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_boards(board_id,project_id,name,description,state) VALUES(?,?,'Topic Watch Board','','active')")
        .bind(board_id.as_str())
        .bind(project_id.as_str())
        .execute(&mut original)
        .await
        .unwrap();
    sqlx::query("INSERT INTO board_topics(topic_id,board_id,name,description) VALUES(?,?,'Topic Watch Topic','')")
        .bind(topic_id.as_str())
        .bind(board_id.as_str())
        .execute(&mut original)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO board_messages(message_id,topic_id,board_id,root_id,actor_key,text) \
         VALUES(?,?,?,NULL,?,'topic watch migration root')",
    )
    .bind(root_id.as_str())
    .bind(topic_id.as_str())
    .bind(board_id.as_str())
    .bind(owner_key)
    .execute(&mut original)
    .await
    .unwrap();
    sqlx::query("INSERT INTO board_threads(root_id,state) VALUES(?,'unresolved')")
        .bind(root_id.as_str())
        .execute(&mut original)
        .await
        .unwrap();
    sqlx::query("UPDATE activity_checkpoint SET last_sequence=1 WHERE singleton=1")
        .execute(&mut original)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) \
         VALUES(1,?,?,?,NULL,'mainMessageCreated',?,?)",
    )
    .bind(project_id.as_str())
    .bind(board_id.as_str())
    .bind(topic_id.as_str())
    .bind(owner_key)
    .bind(root_id.as_str())
    .execute(&mut original)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO topic_watches(reader_key,topic_id,starts_after_activity,active) \
         VALUES(?,?,1,1),(?,?,1,0)",
    )
    .bind(active_reader_key)
    .bind(topic_id.as_str())
    .bind(inactive_reader_key)
    .bind(topic_id.as_str())
    .execute(&mut original)
    .await
    .unwrap();

    let rows_before_migration = topic_watch_row_bytes(&mut original).await;
    original.close().await.unwrap();
    std::fs::copy(&original_path, &copied_path).unwrap();

    BoardStore::open(&copied_path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();

    let mut migrated = SqliteConnection::connect(&format!("sqlite://{}", copied_path.display()))
        .await
        .unwrap();
    let rows_after_migration = topic_watch_row_bytes(&mut migrated).await;
    assert_eq!(rows_after_migration, rows_before_migration);
    let mut foreign_key_targets = sqlx::query("PRAGMA foreign_key_list(topic_watches)")
        .fetch_all(&mut migrated)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("table"))
        .collect::<Vec<_>>();
    foreign_key_targets.sort();
    assert_eq!(
        foreign_key_targets,
        vec!["board_identities", "board_topics"]
    );
    let topic_watch_schema: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='topic_watches'",
    )
    .fetch_one(&mut migrated)
    .await
    .unwrap();
    assert!(topic_watch_schema.contains("CHECK(active IN (0,1))"));
    assert!(topic_watch_schema.ends_with("STRICT"));
    let mut topic_watch_columns = sqlx::query("PRAGMA table_info(topic_watches)")
        .fetch_all(&mut migrated)
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("name"),
                row.get::<i64, _>("notnull"),
                row.get::<i64, _>("pk"),
            )
        })
        .collect::<Vec<_>>();
    topic_watch_columns.sort_by_key(|(name, _, _)| name.clone());
    assert_eq!(
        topic_watch_columns,
        vec![
            ("active".to_owned(), 1, 0),
            ("reader_key".to_owned(), 1, 1),
            ("starts_after_activity".to_owned(), 1, 0),
            ("topic_id".to_owned(), 1, 2),
        ]
    );
    assert!(foreign_key_violations(&mut migrated).await.is_empty());
    assert_eq!(
        migration_versions(&mut migrated).await.last(),
        Some(&202610020001)
    );
    migrated.close().await.unwrap();

    std::fs::remove_file(original_path).unwrap();
    std::fs::remove_file(copied_path).unwrap();
}

#[tokio::test]
async fn participant_migration_preserves_populated_board_and_enforces_one_open_orchestrator() {
    let path = std::path::PathBuf::from("/tmp").join(format!(
        "board-participant-migration-{}.sqlite",
        message_board::MessageId::generate().as_str()
    ));
    let mut connection =
        SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
    initialize_with(
        &mut connection,
        &pre_participants_migrator(),
        &format!("{BASELINE} {THREAD_DELIVERY_POSITIONS}"),
    )
    .await
    .unwrap();
    let project_id = message_board::ProjectId::generate();
    let board_id = message_board::BoardId::generate();
    let topic_id = message_board::TopicId::generate();
    let root_id = message_board::MessageId::generate();
    let first_key = "human:first";
    let second_key = "human:second";
    sqlx::query("INSERT INTO board_identities(identity_key,kind,human_id) VALUES(?,'human','first'),(?,'human','second')")
        .bind(first_key).bind(second_key).execute(&mut connection).await.unwrap();
    sqlx::query("INSERT INTO board_projects(project_id,name,description) VALUES(?,'Project','')")
        .bind(project_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_boards(board_id,project_id,name,description,state) VALUES(?,?,'Board','','active')")
        .bind(board_id.as_str()).bind(project_id.as_str()).execute(&mut connection).await.unwrap();
    sqlx::query(
        "INSERT INTO board_topics(topic_id,board_id,name,description) VALUES(?,?,'Topic','')",
    )
    .bind(topic_id.as_str())
    .bind(board_id.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query("INSERT INTO board_messages(message_id,topic_id,board_id,root_id,actor_key,text) VALUES(?,?,?,NULL,?,'Root')")
        .bind(root_id.as_str()).bind(topic_id.as_str()).bind(board_id.as_str()).bind(first_key)
        .execute(&mut connection).await.unwrap();
    sqlx::query("INSERT INTO board_threads(root_id,state) VALUES(?,'unresolved')")
        .bind(root_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("UPDATE activity_checkpoint SET last_sequence=1 WHERE singleton=1")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) VALUES(1,?,?,?,NULL,'mainMessageCreated',?,?)")
        .bind(project_id.as_str()).bind(board_id.as_str()).bind(topic_id.as_str()).bind(first_key).bind(root_id.as_str())
        .execute(&mut connection).await.unwrap();
    connection.close().await.unwrap();
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
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM board_messages")
            .fetch_one(&mut connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        migration_versions(&mut connection).await,
        vec![
            202609120001,
            202609140001,
            202609150001,
            202609160001,
            202609160002,
            202609170001,
            202610010001,
            202610020001
        ]
    );
    assert!(index_exists(&mut connection, "thread_single_orchestrator").await);
    sqlx::query("UPDATE activity_checkpoint SET last_sequence=3 WHERE singleton=1")
        .execute(&mut connection)
        .await
        .unwrap();
    for (sequence, key) in [(2_i64, first_key), (3_i64, second_key)] {
        sqlx::query("INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) VALUES(?,?,?,?,?,'participantJoined',?,NULL)")
            .bind(sequence).bind(project_id.as_str()).bind(board_id.as_str()).bind(topic_id.as_str()).bind(root_id.as_str()).bind(key)
            .execute(&mut connection).await.unwrap();
    }
    sqlx::query("INSERT INTO thread_participants(reader_key,root_id,role,joined_at_activity,last_seen_activity) VALUES(?,?,'orchestrator',2,2)")
        .bind(first_key).bind(root_id.as_str()).execute(&mut connection).await.unwrap();
    assert!(sqlx::query("INSERT INTO thread_participants(reader_key,root_id,role,joined_at_activity,last_seen_activity) VALUES(?,?,'orchestrator',3,3)")
        .bind(second_key).bind(root_id.as_str()).execute(&mut connection).await.is_err());
    connection.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
