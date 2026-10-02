#![allow(clippy::unwrap_used)]
use crate::participant_history_test_support::*;
use message_board::ParticipantRole::*;

#[tokio::test]
async fn participant_history_writes_all_lifecycle_subjects_and_stable_attribution() {
    let mut fixture = HistoryFixture::open().await;
    let creator = fixture.create(session("O"), Some(Orchestrator)).await;
    let root = creator.message_id.clone();
    let create_join = fixture.latest().await;
    assert_change(
        &mut fixture.store,
        create_join,
        Some(session("O")),
        Some(Orchestrator),
        None,
    )
    .await;
    assert_eq!(
        posted_from(&mut fixture.store, &root).await,
        Some(create_join)
    );
    let reviewer = fixture.join(&root, session("A"), Reviewer, None).await;
    let before = fixture.post(&root, session("A")).await;
    let handover = fixture
        .leave(&root, session("O"), Some(session("A")), false)
        .await;
    let after = fixture.post(&root, session("A")).await;
    assert_change(
        &mut fixture.store,
        handover,
        Some(session("A")),
        Some(Orchestrator),
        Some(session("O")),
    )
    .await;
    let replacement = fixture
        .join(&root, session("O"), Orchestrator, Some(session("A")))
        .await;
    assert_change(
        &mut fixture.store,
        replacement,
        Some(session("O")),
        Some(Orchestrator),
        Some(session("A")),
    )
    .await;
    let advisor = fixture.join(&root, session("A"), Advisor, None).await;
    let advisor_reply = fixture.post(&root, session("A")).await;
    let implementer = fixture.join(&root, session("B"), Implementer, None).await;
    let replace_impl = fixture
        .join(&root, session("A"), Implementer, Some(session("B")))
        .await;
    let impl_reply = fixture.post(&root, session("A")).await;
    let left = fixture.leave(&root, session("A"), None, false).await;
    assert_change(
        &mut fixture.store,
        reviewer,
        Some(session("A")),
        Some(Reviewer),
        None,
    )
    .await;
    assert_change(
        &mut fixture.store,
        advisor,
        Some(session("A")),
        Some(Advisor),
        None,
    )
    .await;
    assert_change(
        &mut fixture.store,
        implementer,
        Some(session("B")),
        Some(Implementer),
        None,
    )
    .await;
    assert_change(
        &mut fixture.store,
        replace_impl,
        Some(session("A")),
        Some(Implementer),
        Some(session("B")),
    )
    .await;
    assert_change(&mut fixture.store, left, Some(session("A")), None, None).await;
    let resolved = fixture.leave(&root, session("O"), None, true).await;
    assert_change(&mut fixture.store, resolved, None, None, None).await;
    let unresolved = fixture.unresolve(&root).await;
    assert_change(&mut fixture.store, unresolved, None, None, None).await;
    let rejoin = fixture.join(&root, session("A"), Participant, None).await;
    let rejoined = fixture.post(&root, session("A")).await;
    let direct_resolve = fixture.resolve(&root, human("owner")).await;
    assert_change(&mut fixture.store, direct_resolve, None, None, None).await;
    for (message, expected) in [
        (&before, reviewer),
        (&after, handover),
        (&advisor_reply, advisor),
        (&impl_reply, replace_impl),
        (&rejoined, rejoin),
    ] {
        assert_eq!(
            posted_from(&mut fixture.store, &message.message_id).await,
            Some(expected)
        );
    }
}

#[tokio::test]
async fn participant_history_writes_human_posts_without_attribution_even_when_joined() {
    let mut fixture = HistoryFixture::open().await;
    let root = fixture.create(human("H"), Some(Reviewer)).await;
    assert_eq!(
        posted_from(&mut fixture.store, &root.message_id).await,
        None
    );
    let reply = fixture.post(&root.message_id, human("H")).await;
    assert_eq!(
        posted_from(&mut fixture.store, &reply.message_id).await,
        None
    );
}

#[tokio::test]
async fn participant_history_live_and_backfilled_reply_agree_on_same_rows() {
    let mut fixture = HistoryFixture::open().await;
    let root = fixture.create(human("H"), None).await.message_id;
    fixture.join(&root, session("A"), Reviewer, None).await;
    fixture.join(&root, session("O"), Orchestrator, None).await;
    let grant = fixture
        .leave(&root, session("O"), Some(session("A")), false)
        .await;
    let reply = fixture.post(&root, session("A")).await;
    let before = posted_from(&mut fixture.store, &reply.message_id).await;
    assert_eq!(before, Some(grant));
    sqlx::raw_sql("UPDATE board_activity SET participant_key=NULL,participant_role=NULL,replaced_participant_key=NULL; UPDATE board_messages SET posted_from_activity=NULL;").execute(&mut fixture.store.connection).await.unwrap();
    let migration = include_str!("../migrations/202610020001_participant_history.sql");
    let backfill = migration.split_once("-- Step 1:").unwrap().1;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("-- Step 1:{backfill}")))
        .execute(&mut fixture.store.connection)
        .await
        .unwrap();
    assert_eq!(
        posted_from(&mut fixture.store, &reply.message_id).await,
        before
    );
}
