#![allow(clippy::unwrap_used)]
use super::participant_history_legacy_support::*;
use message_board::ParticipantRole::*;

#[tokio::test]
async fn participant_history_migration_proves_only_surviving_grants() {
    let mut history = LegacyHistory::open().await;
    let [root, other] = history.roots.clone();
    let no_grant = history.reply(&root, "A").await;
    let overwritten = history.join(&root, "A", Reviewer, None).await;
    let old_reply = history.reply(&root, "A").await;
    let latest = history.join(&root, "A", Advisor, None).await;
    let known_reply = history.reply(&root, "A").await;
    let other_join = history.join(&other, "A", Participant, None).await;
    let other_reply = history.reply(&other, "A").await;
    let human_join = history.join(&root, "H", Reviewer, None).await;
    let human_reply = history.reply(&root, "H").await;
    let left = history.leave(&root, "H").await;
    let resolved = history.resolve(&other, "H").await;
    let (mut store, path) = history.migrate_copy().await;
    for (sequence, expected) in [
        (overwritten, (Some("A"), None, None)),
        (latest, (Some("A"), Some("advisor"), None)),
        (other_join, (Some("A"), Some("participant"), None)),
        (human_join, (Some("H"), Some("reviewer"), None)),
        (left, (Some("H"), None, None)),
        (resolved, (None, None, None)),
    ] {
        assert_event(&mut store, sequence, expected).await;
    }
    for (message, expected) in [
        (&no_grant, None),
        (&old_reply, None),
        (&known_reply, Some(latest)),
        (&other_reply, Some(other_join)),
        (&human_reply, None),
        (&root, None),
        (&other, None),
    ] {
        assert_eq!(attribution(&mut store, message).await, expected);
    }
    finish(store, path).await;
}

#[tokio::test]
async fn participant_history_migration_review_probes_p1_p5_block_stale_roles() {
    let mut history = LegacyHistory::open().await;
    let [p1, p5] = history.roots.clone();
    history.join(&p1, "B", Implementer, None).await;
    let replacement = history.join(&p1, "A", Implementer, Some("B")).await;
    let overwritten = history.join(&p1, "A", Reviewer, None).await;
    let reply1 = history.reply(&p1, "A").await;
    history.join(&p1, "A", Advisor, None).await;
    history.join(&p5, "O", Orchestrator, None).await;
    let promoted_join = history.join(&p5, "A", Reviewer, None).await;
    let handover = history.handover(&p5, "O", "A").await;
    history.resolve(&p5, "A").await;
    history.unresolve(&p5).await;
    history.join(&p5, "A", Reviewer, None).await;
    let reply5 = history.reply(&p5, "A").await;
    history.join(&p5, "A", Advisor, None).await;
    let (mut store, path) = history.migrate_copy().await;
    assert_event(
        &mut store,
        replacement,
        (Some("A"), Some("implementer"), Some("B")),
    )
    .await;
    assert_event(&mut store, overwritten, (Some("A"), None, None)).await;
    assert_event(&mut store, promoted_join, (Some("A"), None, None)).await;
    assert_event(
        &mut store,
        handover,
        (Some("A"), Some("orchestrator"), Some("O")),
    )
    .await;
    assert_eq!(attribution(&mut store, &reply1).await, None);
    assert_eq!(attribution(&mut store, &reply5).await, None);
    finish(store, path).await;
}

#[tokio::test]
async fn participant_history_migration_review_probes_p2_p3_p4_preserve_unknown() {
    let mut history = LegacyHistory::open().await;
    let [p2, p4] = history.roots.clone();
    let mut unknown_events = Vec::new();
    let mut replies = Vec::new();
    for (root, replacement) in [(&p2, false), (&p4, true)] {
        if replacement {
            history.join(root, "B", Implementer, None).await;
            history.join(root, "A", Implementer, Some("B")).await;
        }
        let reviewer = history.join(root, "A", Reviewer, None).await;
        history.join(root, "O", Orchestrator, None).await;
        let unknown = history.handover(root, "O", "A").await;
        history.join(root, "O", Reviewer, None).await;
        replies.push(history.reply(root, "A").await);
        unknown_events.push((reviewer, unknown));
    }
    // P3: legacy session root followed by first join must stay unattributed.
    let (mut store, path) = history.migrate_copy().await;
    for (reviewer, unknown) in unknown_events {
        assert_event(&mut store, reviewer, (Some("A"), None, None)).await;
        assert_event(&mut store, unknown, (None, None, None)).await;
    }
    for message in replies.iter().chain([&p2, &p4]) {
        assert_eq!(attribution(&mut store, message).await, None);
    }
    finish(store, path).await;
}
