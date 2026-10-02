#![allow(clippy::unwrap_used)]
//! Pre-history fixture: actual legacy schema and surviving Participant rows.
use super::*;
use message_board::{MessageId, ParticipantRole};
use sqlx::migrate::Migrator;
use std::borrow::Cow;
use std::path::PathBuf;

pub(super) struct LegacyHistory {
    pub connection: SqliteConnection,
    pub path: PathBuf,
    pub roots: [String; 2],
    sequence: i64,
}
impl LegacyHistory {
    pub async fn open() -> Self {
        let path = std::env::temp_dir().join(format!(
            "participant-history-{}.sqlite",
            uuid::Uuid::now_v7()
        ));
        let mut connection =
            SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", path.display()))
                .await
                .unwrap();
        let sources: [(i64, &str); 7] = [
            (202609120001, BASELINE),
            (202609140001, THREAD_DELIVERY_POSITIONS),
            (202609150001, THREAD_PARTICIPANTS),
            (202609160001, THREAD_IMPLEMENTER),
            (202609160002, TOPIC_WATCHES),
            (202609170001, THREAD_SUBSCRIPTIONS),
            (202610010001, TOPIC_WATCHES_EMPTY_BOUNDARY),
        ];
        // Use original descriptions/checksums, allowing the real migrator to reopen it.
        let migrations = Migrator {
            migrations: Cow::Owned(
                MIGRATOR
                    .iter()
                    .filter(|m| m.version <= 202610010001)
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        };
        let schema = sources
            .iter()
            .map(|(_, sql)| *sql)
            .collect::<Vec<_>>()
            .join(" ");
        initialize_with(&mut connection, &migrations, &schema)
            .await
            .unwrap();
        for label in ["A", "B", "O"] {
            sqlx::query("INSERT INTO board_identities(identity_key,kind,service_id,endpoint_id,session_id) VALUES(?,'session','550e8400-e29b-41d4-a716-446655440000','codex-local',?)").bind(legacy_key(label)).bind(label).execute(&mut connection).await.unwrap();
        }
        sqlx::raw_sql("INSERT INTO board_identities(identity_key,kind,human_id) VALUES('human:H','human','H'); INSERT INTO board_projects VALUES('550e8400-e29b-41d4-a716-446655440001','Project',''); INSERT INTO project_boards VALUES('550e8400-e29b-41d4-a716-446655440002','550e8400-e29b-41d4-a716-446655440001','Board','','active'); INSERT INTO board_topics VALUES('550e8400-e29b-41d4-a716-446655440003','550e8400-e29b-41d4-a716-446655440002','Topic','');").execute(&mut connection).await.unwrap();
        let roots = [
            MessageId::generate().as_str().to_owned(),
            MessageId::generate().as_str().to_owned(),
        ];
        let mut fixture = Self {
            connection,
            path,
            roots,
            sequence: 0,
        };
        for root in fixture.roots.clone() {
            fixture.message(&root, None, "A").await;
            sqlx::query("INSERT INTO board_threads VALUES(?,'unresolved')")
                .bind(root)
                .execute(&mut fixture.connection)
                .await
                .unwrap();
        }
        fixture
    }
    pub async fn event(&mut self, root: &str, kind: &str, actor: &str) -> i64 {
        self.sequence += 1;
        sqlx::query("UPDATE activity_checkpoint SET last_sequence=?")
            .bind(self.sequence)
            .execute(&mut self.connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO board_activity VALUES(?,'550e8400-e29b-41d4-a716-446655440001','550e8400-e29b-41d4-a716-446655440002','550e8400-e29b-41d4-a716-446655440003',?,?,?,NULL)")
            .bind(self.sequence)
            .bind(root)
            .bind(kind)
            .bind(legacy_key(actor))
            .execute(&mut self.connection)
            .await
            .unwrap();
        self.sequence
    }
    pub async fn join(
        &mut self,
        root: &str,
        actor: &str,
        role: ParticipantRole,
        replace: Option<&str>,
    ) -> i64 {
        let role = crate::participant_row_decoding::role_name(role);
        let kind = match (replace, role) {
            (Some(_), "orchestrator") => "orchestratorReplaced",
            (Some(_), _) => "implementerReplaced",
            _ => "participantJoined",
        };
        let sequence = self.event(root, kind, actor).await;
        if let Some(holder) = replace {
            sqlx::query("UPDATE thread_participants SET closed_at_activity=?,last_seen_activity=?,closed_reason='replaced',replaced_by=? WHERE root_id=? AND reader_key=?").bind(sequence).bind(sequence).bind(legacy_key(actor)).bind(root).bind(legacy_key(holder)).execute(&mut self.connection).await.unwrap();
        }
        sqlx::query("INSERT INTO thread_participants(reader_key,root_id,role,joined_at_activity,last_seen_activity) VALUES(?,?,?,?,?) ON CONFLICT(reader_key,root_id) DO UPDATE SET role=excluded.role,joined_at_activity=excluded.joined_at_activity,last_seen_activity=excluded.last_seen_activity,closed_at_activity=NULL,closed_reason=NULL,replaced_by=NULL").bind(legacy_key(actor)).bind(root).bind(role).bind(sequence).bind(sequence).execute(&mut self.connection).await.unwrap();
        sequence
    }
    pub async fn handover(&mut self, root: &str, actor: &str, target: &str) -> i64 {
        let sequence = self.event(root, "orchestratorReplaced", actor).await;
        sqlx::query("UPDATE thread_participants SET closed_at_activity=?,last_seen_activity=?,closed_reason='replaced',replaced_by=? WHERE reader_key=? AND root_id=?").bind(sequence).bind(sequence).bind(legacy_key(target)).bind(legacy_key(actor)).bind(root).execute(&mut self.connection).await.unwrap();
        sqlx::query("UPDATE thread_participants SET role='orchestrator',last_seen_activity=? WHERE reader_key=? AND root_id=?").bind(sequence).bind(legacy_key(target)).bind(root).execute(&mut self.connection).await.unwrap();
        sequence
    }
    pub async fn leave(&mut self, root: &str, actor: &str) -> i64 {
        let sequence = self.event(root, "participantLeft", actor).await;
        sqlx::query("UPDATE thread_participants SET closed_at_activity=?,last_seen_activity=?,closed_reason='left',replaced_by=NULL WHERE reader_key=? AND root_id=?").bind(sequence).bind(sequence).bind(legacy_key(actor)).bind(root).execute(&mut self.connection).await.unwrap();
        sequence
    }
    pub async fn resolve(&mut self, root: &str, actor: &str) -> i64 {
        let sequence = self.event(root, "threadResolved", actor).await;
        sqlx::query("UPDATE thread_participants SET closed_at_activity=?,last_seen_activity=?,closed_reason='resolved',replaced_by=NULL WHERE root_id=? AND closed_at_activity IS NULL").bind(sequence).bind(sequence).bind(root).execute(&mut self.connection).await.unwrap();
        sqlx::query("UPDATE board_threads SET state='resolved' WHERE root_id=?")
            .bind(root)
            .execute(&mut self.connection)
            .await
            .unwrap();
        sequence
    }
    pub async fn unresolve(&mut self, root: &str) {
        self.event(root, "threadUnresolved", "H").await;
        sqlx::query("UPDATE board_threads SET state='unresolved' WHERE root_id=?")
            .bind(root)
            .execute(&mut self.connection)
            .await
            .unwrap();
    }
    pub async fn reply(&mut self, root: &str, actor: &str) -> String {
        let message = MessageId::generate().as_str().to_owned();
        self.message(&message, Some(root), actor).await;
        message
    }
    async fn message(&mut self, message: &str, root: Option<&str>, actor: &str) {
        sqlx::query("INSERT INTO board_messages(message_id,topic_id,board_id,root_id,actor_key,text) VALUES(?,'550e8400-e29b-41d4-a716-446655440003','550e8400-e29b-41d4-a716-446655440002',?,?,'history')").bind(message).bind(root).bind(legacy_key(actor)).execute(&mut self.connection).await.unwrap();
        self.sequence += 1;
        sqlx::query("UPDATE activity_checkpoint SET last_sequence=?")
            .bind(self.sequence)
            .execute(&mut self.connection)
            .await
            .unwrap();
        let kind = if root.is_some() {
            "threadMessageCreated"
        } else {
            "mainMessageCreated"
        };
        sqlx::query("INSERT INTO board_activity VALUES(?,'550e8400-e29b-41d4-a716-446655440001','550e8400-e29b-41d4-a716-446655440002','550e8400-e29b-41d4-a716-446655440003',?,?,?,?)")
            .bind(self.sequence)
            .bind(root)
            .bind(kind)
            .bind(legacy_key(actor))
            .bind(message)
            .execute(&mut self.connection)
            .await
            .unwrap();
    }
    pub async fn migrate_copy(self) -> (crate::BoardStore, PathBuf) {
        let copied = self.path.with_extension("copy.sqlite");
        self.connection.close().await.unwrap();
        std::fs::copy(&self.path, &copied).unwrap();
        std::fs::remove_file(self.path).unwrap();
        (crate::BoardStore::open(&copied).await.unwrap(), copied)
    }
}

pub(super) async fn assert_event(
    store: &mut crate::BoardStore,
    sequence: i64,
    expected: (Option<&str>, Option<&str>, Option<&str>),
) {
    let actual: (Option<String>, Option<String>, Option<String>) = sqlx::query_as("SELECT participant_key,participant_role,replaced_participant_key FROM board_activity WHERE activity_sequence=?").bind(sequence).fetch_one(&mut store.connection).await.unwrap();
    assert_eq!(
        actual,
        (
            expected.0.map(legacy_key),
            expected.1.map(str::to_owned),
            expected.2.map(legacy_key)
        ),
        "event {sequence}"
    );
}
pub(super) async fn attribution(store: &mut crate::BoardStore, message: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT posted_from_activity FROM board_messages WHERE message_id=?")
        .bind(message)
        .fetch_one(&mut store.connection)
        .await
        .unwrap()
}
pub(super) async fn finish(mut store: crate::BoardStore, path: PathBuf) {
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut store.connection)
            .await
            .unwrap()
            .is_empty()
    );
    store.close().await.unwrap();
    crate::BoardStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    std::fs::remove_file(path).unwrap();
}

pub(super) fn legacy_key(label: &str) -> String {
    if label == "H" {
        "human:H".to_owned()
    } else {
        format!("session:550e8400-e29b-41d4-a716-446655440000:codex-local:{label}")
    }
}
