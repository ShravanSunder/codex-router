//! Durable settlement, retention and revision-cache interleavings.
use super::super::super::{
    CachePublicationPause, InteractionHistoryError, InteractionHistoryState, QuestionHistoryState,
    QuestionResponse,
};
use super::*;
use std::{sync::Arc, time::Duration as WaitDuration};

#[tokio::test]
async fn mixed_typed_import_preserves_content_and_settlement_never_resets_creation_time() {
    let directory = tempfile::tempdir().expect("mixed typed import");
    let source = directory.path().join("interaction-history.json");
    let requester = session_ref("requester");
    let approver = Identity::Session {
        session: session_ref("approver"),
    };
    let approval = InteractionHistoryRecord::Approval {
        requester: requester.clone(),
        approver: approver.clone(),
        request: serde_json::from_value(
            serde_json::json!({"requestId":"approval","title":"Run?", "options":[
                {"optionId":"allow-once","label":"Allow","choice":{"effect":"allow","scope":"once"}}
            ]}),
        )
        .expect("approval"),
        state: InteractionHistoryState::Pending,
        legacy_metadata: None,
    };
    let pending_question = InteractionHistoryRecord::Question {
        requester,
        approver: approver.clone(),
        request: question("question"),
        state: QuestionHistoryState::Pending,
    };
    let mut dated_approval = serde_json::to_value(&approval).expect("approval source");
    dated_approval["createdAt"] = "2026-09-18T00:00:00.000000007Z".into();
    let input = serde_json::json!({"approval":dated_approval,"question":pending_question,
        "refusal":refused_approval("refusal")});
    let original = serde_json::to_vec_pretty(&input).expect("mixed source");
    tokio::fs::write(&source, &original).await.expect("source");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("whole mixed import");
    assert_eq!(
        serde_json::to_value(store.interaction("approval").await.expect("approval row"))
            .expect("approval value"),
        serde_json::to_value(approval).expect("original approval")
    );
    assert_eq!(
        serde_json::to_value(store.interaction("question").await.expect("question row"))
            .expect("question value"),
        input["question"]
    );
    assert_eq!(
        serde_json::to_value(store.interaction("refusal").await.expect("refusal row"))
            .expect("refusal value"),
        input["refusal"]
    );
    let timestamps = store.data.lock().await.created_at.clone();
    assert_eq!(
        timestamps["approval"].to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        "2026-09-18T00:00:00.000000007Z"
    );
    assert_eq!(
        timestamps["question"], timestamps["refusal"],
        "undated rows stamp once at the same import instant"
    );
    store
        .decide("approval", &approver, "allow-once", false)
        .await
        .expect("settlement");
    store
        .respond_question("question", &approver, &QuestionResponse::Declined)
        .await
        .expect("declined question");
    assert_eq!(store.data.lock().await.created_at, timestamps);
    drop(store);
    let reopened = InteractionHistoryStore::load(source.clone())
        .await
        .expect("reopen mixed terminals");
    assert_eq!(reopened.data.lock().await.created_at, timestamps);
    assert!(matches!(reopened.interaction("approval").await,
        Some(InteractionHistoryRecord::Approval { state: InteractionHistoryState::Decided { option_id }, .. })
        if option_id.as_str()=="allow-once"));
    assert_eq!(
        tokio::fs::read(source).await.expect("original recovery"),
        original
    );
}

#[tokio::test]
async fn retention_removes_oldest_then_id_in_bounded_batches_and_keeps_exact_cutoff() {
    let directory = tempfile::tempdir().expect("retention ordering fixture");
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-08T00:00:00.000000000Z")
        .expect("clock")
        .with_timezone(&Utc);
    let cutoff = now - Duration::days(30);
    let fixtures = [
        ("old-z", cutoff - Duration::nanoseconds(2)),
        ("old-a", cutoff - Duration::nanoseconds(1)),
        ("old-b", cutoff - Duration::nanoseconds(1)),
        ("old-y", cutoff - Duration::nanoseconds(3)),
        ("exact", cutoff),
        ("recent", now),
    ];
    let mut input = serde_json::Map::new();
    for (request_id, created_at) in fixtures {
        let mut record = serde_json::to_value(refused_approval(request_id)).expect("record");
        record["createdAt"] = created_at
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
            .into();
        input.insert(request_id.into(), record);
    }
    let original = serde_json::to_vec(&input).expect("retention source");
    let source = directory.path().join("interaction-history.json");
    tokio::fs::write(&source, &original).await.expect("source");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("store");
    assert!(store.prune_expired(now, 0).await.is_err());
    assert!(store.prune_expired(now, 501).await.is_err());
    assert_eq!(store.prune_expired(now, 2).await.expect("bounded prune"), 2);
    assert!(store.interaction("old-y").await.is_none());
    assert!(store.interaction("old-z").await.is_none());
    assert!(store.interaction("old-a").await.is_some());
    assert!(store.interaction("old-b").await.is_some());
    assert_eq!(store.prune_expired(now, 1).await.expect("id tie break"), 1);
    assert!(store.interaction("old-a").await.is_none());
    assert!(store.interaction("old-b").await.is_some());
    assert_eq!(
        store
            .prune_expired(now, 500)
            .await
            .expect("remaining expired"),
        1
    );
    drop(store);
    let reopened = InteractionHistoryStore::load(source.clone())
        .await
        .expect("durable prune");
    assert_eq!(
        reopened
            .list_all()
            .await
            .iter()
            .map(|record| record.request_id())
            .collect::<Vec<_>>(),
        ["exact", "recent"]
    );
    assert_eq!(
        tokio::fs::read(source).await.expect("protected source"),
        original
    );
}

#[tokio::test]
async fn low_level_observer_reopen_does_not_reconcile_live_pending_records() {
    let directory = tempfile::tempdir().expect("pending fixture");
    let source = directory.path().join("interaction-history.json");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("store");
    store
        .record_question(
            session_ref("requester"),
            Identity::Session {
                session: session_ref("approver"),
            },
            question("pending"),
        )
        .await
        .expect("pending question");
    let observer = InteractionHistoryStore::load(source.clone())
        .await
        .expect("low-level reopen");
    assert!(matches!(
        observer.interaction("pending").await,
        Some(InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Pending,
            ..
        })
    ));
    drop(observer);
    drop(store);
    let startup = InteractionHistoryStore::load(source)
        .await
        .expect("owning startup store");
    startup
        .reconcile_pending_on_startup()
        .await
        .expect("explicit owning reconciliation");
    assert!(matches!(startup.interaction("pending").await,
        Some(InteractionHistoryRecord::Question { state: QuestionHistoryState::Cancelled { reason }, .. })
        if reason.as_str()=="hostRestarted"));
}

#[tokio::test]
async fn independent_connections_refresh_before_writing_without_losing_unrelated_rows() {
    let directory = tempfile::tempdir().expect("writer fixture");
    let source = directory.path().join("interaction-history.json");
    let first = InteractionHistoryStore::load(source.clone())
        .await
        .expect("first writer");
    let second = InteractionHistoryStore::load(source.clone())
        .await
        .expect("second storage writer");
    let (left, right) = tokio::join!(add_refusal(&first, "left"), add_refusal(&second, "right"));
    left.expect("left write");
    right.expect("right write");
    let reopened = InteractionHistoryStore::load(source)
        .await
        .expect("independent durable observation");
    assert_eq!(
        reopened
            .list_all()
            .await
            .iter()
            .map(|record| record.request_id())
            .collect::<Vec<_>>(),
        ["left", "right"]
    );
}

#[tokio::test]
async fn independently_stale_settlement_attempts_have_one_durable_winner() {
    let directory = tempfile::tempdir().expect("settlement fixture");
    let source = directory.path().join("interaction-history.json");
    let first = InteractionHistoryStore::load(source.clone())
        .await
        .expect("first writer");
    let approver = Identity::Session {
        session: session_ref("approver"),
    };
    first
        .record_question(
            session_ref("requester"),
            approver.clone(),
            question("question"),
        )
        .await
        .expect("question");
    let second = InteractionHistoryStore::load(source.clone())
        .await
        .expect("stale second writer");
    let answered = QuestionResponse::Answered {
        content: serde_json::from_value(serde_json::json!({"yes":true})).expect("answer"),
    };
    let (left, right) = tokio::join!(
        first.respond_question("question", &approver, &answered),
        second.respond_question("question", &approver, &QuestionResponse::Declined)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(matches!(
        left.as_ref().err().or(right.as_ref().err()),
        Some(InteractionHistoryError::AlreadySettled)
    ));
    let reopened = InteractionHistoryStore::load(source)
        .await
        .expect("durable terminal");
    assert!(matches!(
        reopened.interaction("question").await,
        Some(InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Answered { .. } | QuestionHistoryState::Declined,
            ..
        })
    ));
}

#[tokio::test]
async fn aborted_commit_before_cache_publication_recovers_on_same_store_next_mutation() {
    let directory = tempfile::tempdir().expect("commit/cache fixture");
    let source = directory.path().join("interaction-history.json");
    let store = Arc::new(
        InteractionHistoryStore::load(source.clone())
            .await
            .expect("store"),
    );
    let (committed, observed_commit) = tokio::sync::oneshot::channel();
    let (_resume, resume) = tokio::sync::oneshot::channel();
    *store.cache_publication_pause.lock().await = Some(CachePublicationPause { committed, resume });
    let task_store = store.clone();
    let mutation =
        tokio::spawn(async move { add_refusal(&task_store, "committed-unacknowledged").await });
    tokio::time::timeout(WaitDuration::from_secs(3), observed_commit)
        .await
        .expect("bounded actual commit")
        .expect("commit reached publication gap");
    let mut independent = observer(directory.path()).await;
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM interaction_history_records WHERE request_id='committed-unacknowledged'")
        .fetch_one(&mut independent).await.expect("actual commit visible externally");
    assert_eq!(persisted, 1);
    mutation.abort();
    assert!(mutation.await.expect_err("caller cancelled").is_cancelled());
    assert!(
        store
            .interaction("committed-unacknowledged")
            .await
            .is_none(),
        "cache remains last acknowledged"
    );
    add_refusal(&store, "next-valid-write")
        .await
        .expect("same store accepts new mutation without restart");
    assert!(
        store
            .interaction("committed-unacknowledged")
            .await
            .is_some()
    );
    let reopened = InteractionHistoryStore::load(source)
        .await
        .expect("durable reopen");
    assert_eq!(reopened.list_all().await.len(), 2);
}

#[tokio::test]
async fn reconciliation_failure_after_import_keeps_committed_stamp_for_next_owning_startup() {
    let directory = tempfile::tempdir().expect("import/reconciliation gap");
    let source = directory.path().join("interaction-history.json");
    let pending = InteractionHistoryRecord::Question {
        requester: session_ref("requester"),
        approver: Identity::Session {
            session: session_ref("approver"),
        },
        request: question("pending"),
        state: QuestionHistoryState::Pending,
    };
    let original = serde_json::to_vec(&BTreeMap::from([("pending", pending)])).expect("source");
    tokio::fs::write(&source, &original)
        .await
        .expect("source before first import");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("committed first import");
    let committed_stamp = store.data.lock().await.created_at["pending"];
    let mut connection = observer(directory.path()).await;
    sqlx::query("CREATE TRIGGER reject_reconciliation BEFORE UPDATE ON interaction_history_records BEGIN SELECT RAISE(ABORT, 'fixture reconciliation failure'); END")
        .execute(&mut connection).await.expect("real post-import failure");
    assert!(matches!(
        store.reconcile_pending_on_startup().await,
        Err(InteractionHistoryError::Unavailable)
    ));
    let json: String = sqlx::query_scalar(
        "SELECT record_json FROM interaction_history_records WHERE request_id='pending'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("prior import survived failed reconciliation");
    assert!(matches!(
        serde_json::from_str::<InteractionHistoryRecord>(&json).expect("pending row"),
        InteractionHistoryRecord::Question {
            state: QuestionHistoryState::Pending,
            ..
        }
    ));
    sqlx::query("DROP TRIGGER reject_reconciliation")
        .execute(&mut connection)
        .await
        .expect("restore fixture");
    drop(store);
    let next_startup = InteractionHistoryStore::load(source.clone())
        .await
        .expect("no second import");
    next_startup
        .reconcile_pending_on_startup()
        .await
        .expect("next owning reconciliation");
    assert_eq!(
        next_startup.data.lock().await.created_at["pending"],
        committed_stamp
    );
    assert!(matches!(next_startup.interaction("pending").await,
        Some(InteractionHistoryRecord::Question {state:QuestionHistoryState::Cancelled {reason},..})
        if reason.as_str()=="hostRestarted"));
    assert_eq!(
        tokio::fs::read(source)
            .await
            .expect("original source retained"),
        original
    );
}

#[tokio::test]
async fn revision_overflow_rejects_mutation_without_wrapping_or_committing_rows() {
    let directory = tempfile::tempdir().expect("revision overflow fixture");
    let source = directory.path().join("interaction-history.json");
    let store = InteractionHistoryStore::load(source).await.expect("store");
    let mut independent = observer(directory.path()).await;
    sqlx::query("UPDATE interaction_history_import SET revision=9223372036854775807")
        .execute(&mut independent)
        .await
        .expect("maximal valid stored revision");
    assert!(matches!(
        add_refusal(&store, "must-not-commit").await,
        Err(InteractionHistoryError::Unavailable)
    ));
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM interaction_history_import")
        .fetch_one(&mut independent)
        .await
        .expect("unchanged revision");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM interaction_history_records")
        .fetch_one(&mut independent)
        .await
        .expect("unchanged committed history");
    assert_eq!(revision, i64::MAX);
    assert_eq!(count, 0);
    assert!(store.list_all().await.is_empty());
}

#[tokio::test]
async fn real_sqlite_write_failure_rolls_back_delta_and_retains_acknowledged_cache() {
    let directory = tempfile::tempdir().expect("write failure fixture");
    let source = directory.path().join("interaction-history.json");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("store");
    add_refusal(&store, "retained")
        .await
        .expect("initial committed record");
    let mut connection = observer(directory.path()).await;
    sqlx::query("CREATE TRIGGER reject_history_insert BEFORE INSERT ON interaction_history_records BEGIN SELECT RAISE(ABORT, 'fixture write rejection'); END")
        .execute(&mut connection).await.expect("real SQLite write failure boundary");
    assert!(matches!(
        add_refusal(&store, "failed").await,
        Err(InteractionHistoryError::Unavailable)
    ));
    assert_eq!(store.list_all().await.len(), 1);
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM interaction_history_records")
        .fetch_one(&mut connection)
        .await
        .expect("previous committed set retained");
    assert_eq!(records, 1);
    sqlx::query("DROP TRIGGER reject_history_insert")
        .execute(&mut connection)
        .await
        .expect("restore fixture schema");
    let reopened = InteractionHistoryStore::load(source)
        .await
        .expect("reopen prior committed history");
    assert!(reopened.interaction("failed").await.is_none());
    assert!(reopened.interaction("retained").await.is_some());
}
