//! Real Control proofs for off-board DMs and retained conversation views.
use super::*;
use chrono::{DateTime, Duration, Utc};
use collaboration_protocol::PushRecordNotice;
use collaboration_service::{
    AutomationRetentionWorker, BoardAvailability, MachineIdentity, SubscriptionDeliveryService,
    SubscriptionDeliveryServiceProps, SystemSubscriptionClock,
};
use message_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::PathBuf;

struct BoardControlFixture {
    pushes: Arc<TokioMutex<AutomationStore>>,
    board_path: PathBuf,
    retention: AutomationRetentionWorker,
    control: ControlHarness,
    _directory: tempfile::TempDir,
}

impl BoardControlFixture {
    async fn start() -> Self {
        let directory = tempfile::tempdir().expect("isolated Control databases");
        let board_path = directory.path().join("board.sqlite");
        let board = Arc::new(TokioMutex::new(
            BoardStore::open(&board_path)
                .await
                .expect("real board store"),
        ));
        let pushes = Arc::new(TokioMutex::new(
            AutomationStore::open(&directory.path().join("automation.sqlite"))
                .await
                .expect("real automation store"),
        ));
        let delivery = Arc::new(RecordingDelivery::new(Arc::clone(&pushes)));
        let presence = Arc::new(RunningPresence);
        let owner = SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
            board_availability: BoardAvailability::Available(Arc::clone(&board)),
            push_store: Arc::clone(&pushes),
            delivery: delivery.clone(),
            presence: presence.clone(),
            machine_identity: MachineIdentity::new(
                UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("service UUID"),
                Some("off-board-proof"),
            )
            .expect("machine identity"),
            clock: Arc::new(SystemSubscriptionClock),
        });
        owner.start().await.expect("real reader owner starts");
        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_ID)
            .expect("service identity")
            .with_board_store(board)
            .with_automation_store(Arc::clone(&pushes))
            .with_session_delivery(delivery)
            .with_subscription_delivery_service(owner.clone(), presence);
        let retention = identity
            .automation_retention_worker()
            .expect("real retention worker");
        let mut control = ControlHarness::start(identity).await;
        control.owner = Some(owner);
        Self {
            pushes,
            board_path,
            retention,
            control,
            _directory: directory,
        }
    }

    async fn close(self) {
        let Self {
            pushes,
            retention,
            control,
            _directory,
            ..
        } = self;
        control.close().await;
        drop(retention);
        Arc::try_unwrap(pushes)
            .ok()
            .expect("Control and owner must release automation storage")
            .into_inner()
            .close()
            .await
            .expect("close automation database before its directory");
        drop(_directory);
    }
}

async fn board_content_counts(path: &std::path::Path) -> (i64, i64) {
    let mut observer = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true),
    )
    .await
    .expect("open real board observer");
    let counts = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM board_messages),(SELECT count(*) FROM board_activity)",
    )
    .fetch_one(&mut observer)
    .await
    .expect("count stored board content");
    observer.close().await.expect("close board observer");
    counts
}

#[tokio::test]
async fn control_dm_send_and_read_leave_board_messages_and_activity_empty() {
    let mut fixture = BoardControlFixture::start().await;
    assert_eq!(board_content_counts(&fixture.board_path).await, (0, 0));
    let sender = session("claude-local", "off-board-sender");
    let target = session("codex-local", "off-board-recipient");
    let sent = fixture
        .control
        .call(
            "message/send",
            send_params(&target, &sender, "off-board body"),
        )
        .await;
    assert_eq!(
        sent.pointer("/result/receipt/outcome/kind")
            .and_then(Value::as_str),
        Some("peerMessageWritten")
    );
    let push_id = sent
        .pointer("/result/pushId")
        .and_then(Value::as_str)
        .expect("Control send created a push")
        .to_owned();
    assert_eq!(board_content_counts(&fixture.board_path).await, (0, 0));
    let shown = fixture
        .control
        .call("router/show", json!({"caller":target,"reference":push_id}))
        .await;
    assert_eq!(
        shown.pointer("/result/record/body").and_then(Value::as_str),
        Some("off-board body")
    );
    assert_eq!(board_content_counts(&fixture.board_path).await, (0, 0));
    fixture.close().await;
}

async fn stored_dm(
    fixture: &BoardControlFixture,
    origin: &SessionRef,
    target: &SessionRef,
    created_at: DateTime<Utc>,
    body: &str,
) -> PushId {
    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("UUIDv7 push ID");
    fixture
        .pushes
        .lock()
        .await
        .insert_push_record(PushRecordDraft {
            push_id: push_id.clone(),
            kind: PushKind::DirectMessage,
            origin: PushOrigin::Session(origin.clone()),
            origin_router_ref: None,
            target: target.clone(),
            reply_to_push_id: None,
            header_facts: PushHeaderFacts::DirectMessage {
                sender_display_name: None,
            },
            body: Some(body.to_owned()),
            activity: None,
            mode: Some(MessageDelivery::Auto),
            guard: None,
            created_at,
        })
        .await
        .expect("persist a typed DM in real SQLite");
    push_id
}

fn notices(response: Value) -> Vec<PushRecordNotice> {
    serde_json::from_value(
        response
            .pointer("/result/records")
            .expect("real Control returned notice records")
            .clone(),
    )
    .expect("decode published notice types")
}

fn notice_ids(records: &[PushRecordNotice]) -> Vec<PushId> {
    records
        .iter()
        .map(|record| record.push_id.clone())
        .collect()
}

#[tokio::test]
async fn control_inbox_and_history_order_retained_dms_and_omit_expired_records() {
    let mut fixture = BoardControlFixture::start().await;
    // Maintenance accepts an injected wall time; no real-time wait drives expiry.
    let now = DateTime::parse_from_rfc3339("2026-09-01T12:00:00Z")
        .expect("fixed retention clock")
        .with_timezone(&Utc);
    let cutoff = now - Duration::days(30);
    let caller = session("codex-local", "history-recipient");
    let correspondent = session("claude-local", "history-correspondent");
    let other = session("cursor-local", "unrelated-correspondent");
    let expired = stored_dm(
        &fixture,
        &correspondent,
        &caller,
        cutoff - Duration::milliseconds(1),
        "expired body",
    )
    .await;
    let boundary = stored_dm(
        &fixture,
        &correspondent,
        &caller,
        cutoff,
        "exact thirty-day boundary",
    )
    .await;
    let older = stored_dm(
        &fixture,
        &correspondent,
        &caller,
        now - Duration::days(2),
        "older unread",
    )
    .await;
    let read = stored_dm(
        &fixture,
        &correspondent,
        &caller,
        now - Duration::days(1),
        "read conversation message",
    )
    .await;
    let outbound = stored_dm(
        &fixture,
        &caller,
        &correspondent,
        now - Duration::hours(3),
        "outgoing conversation message",
    )
    .await;
    let newest = stored_dm(
        &fixture,
        &correspondent,
        &caller,
        now - Duration::hours(2),
        "newest incoming",
    )
    .await;
    let unrelated = stored_dm(
        &fixture,
        &other,
        &caller,
        now - Duration::hours(1),
        "other correspondent",
    )
    .await;
    let shown = fixture
        .control
        .call("router/show", json!({"caller":caller,"reference":read}))
        .await;
    assert!(
        shown
            .pointer("/result/record/readAt")
            .is_some_and(|value| !value.is_null())
    );
    assert_eq!(
        fixture
            .retention
            .prune_batch(now.timestamp_millis())
            .await
            .expect("real retained-content maintenance"),
        1
    );
    assert!(
        fixture
            .pushes
            .lock()
            .await
            .get_push_record(&expired)
            .await
            .expect("inspect expired ID")
            .is_none()
    );
    let inbox = notices(
        fixture
            .control
            .call("message/inbox", json!({"caller":caller,"limit":50}))
            .await,
    );
    assert_eq!(
        notice_ids(&inbox),
        vec![unrelated, newest.clone(), older.clone(), boundary.clone()]
    );
    assert!(inbox.iter().all(|record| record.read_at.is_none()
        && record.target == caller
        && record.created_at >= cutoff));
    let history = notices(
        fixture
            .control
            .call(
                "message/history",
                json!({"caller":caller,"with":correspondent,"limit":50}),
            )
            .await,
    );
    assert_eq!(
        notice_ids(&history),
        vec![newest, outbound, read, older, boundary]
    );
    assert!(history.iter().all(|record| record.created_at >= cutoff));
    assert!(history.iter().any(|record| record.target == correspondent));
    assert!(history.iter().any(|record| record.read_at.is_some()));
    assert_eq!(board_content_counts(&fixture.board_path).await, (0, 0));
    fixture.close().await;
}
