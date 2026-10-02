#![allow(clippy::unwrap_used)]
use super::participant_history_legacy_support::*;
use crate::participant_history_test_support::{post, session};
use message_board::{ParticipantRole::*, *};

#[tokio::test]
async fn participant_history_upgrade_p2_p4_post_unknown_then_recover_after_join() {
    let mut history = LegacyHistory::open().await;
    let [p2, p4] = history.roots.clone();
    for (root, with_older_grant) in [(&p2, false), (&p4, true)] {
        if with_older_grant {
            history.join(root, "B", Implementer, None).await;
            history.join(root, "A", Implementer, Some("B")).await;
        }
        history.join(root, "A", Reviewer, None).await;
        history.join(root, "O", Orchestrator, None).await;
        history.handover(root, "O", "A").await;
        history.join(root, "O", Reviewer, None).await;
    }
    let (mut store, path) = history.migrate_copy().await;
    for root in [p2, p4] {
        let root: MessageId = root.try_into().unwrap();
        let unknown = post(&mut store, &root, session("A")).await;
        assert_eq!(
            attribution(&mut store, unknown.message_id.as_str()).await,
            None
        );
        assert!(
            serde_json::to_value(&unknown)
                .unwrap()
                .get("postedAsRole")
                .is_none()
        );
        let joined = store
            .join_thread(
                ThreadJoinRequest {
                    root_message_id: root.clone(),
                    actor: session("A"),
                    role: Orchestrator,
                    replace: None,
                    watch: false,
                    note: None,
                    mode: None,
                    when_idle: None,
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        let known = post(&mut store, &root, session("A")).await;
        assert_eq!(
            serde_json::to_value(&known).unwrap()["postedAsRole"],
            "orchestrator"
        );
        let old = store
            .show_message(MessageShowRequest {
                message_id: unknown.message_id.clone(),
            })
            .await
            .unwrap();
        assert!(
            serde_json::to_value(old.0)
                .unwrap()
                .get("postedAsRole")
                .is_none()
        );
        assert_eq!(
            attribution(&mut store, known.message_id.as_str()).await,
            Some(
                joined
                    .participant
                    .joined_at_activity
                    .get()
                    .try_into()
                    .unwrap()
            )
        );
    }
    finish(store, path).await;
}
