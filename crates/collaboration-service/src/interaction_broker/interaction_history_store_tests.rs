use super::InteractionHistoryStore;
use super::super::{
    InteractionHistoryRecord, RefusedApprovalOption, RefusedTypedApproval,
};
use chrono::{Duration, Utc};
use message_board::{Identity, SessionRef};
use std::collections::BTreeMap;

fn session_ref(session_id: &str) -> SessionRef {
    serde_json::from_value(serde_json::json!({
        "endpoint":{
            "serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f",
            "endpointId":"codex-local"
        },
        "sessionId":session_id
    }))
    .expect("valid fixture session")
}

fn refused_approval(request_id: &str) -> InteractionHistoryRecord {
    let requester = session_ref("requester");
    InteractionHistoryRecord::RefusedApproval {
        requester: requester.clone(),
        approver: Identity::Session {
            session: requester,
        },
        refusal: RefusedTypedApproval {
            request_id: request_id.to_owned(),
            title: "Rejected approval".to_owned(),
            description: None,
            subject: None,
            options: vec![RefusedApprovalOption {
                option_id: "reject".to_owned(),
                label: "Reject".to_owned(),
                provider_kind: "fixture".to_owned(),
            }],
            reason: "fixture refusal".to_owned(),
        },
    }
}

#[tokio::test]
async fn load_stamps_undated_entries_and_prunes_strictly_after_thirty_days() {
    let directory = tempfile::tempdir().expect("isolated interaction history");
    let path = directory.path().join("interaction-history.json");
    let legacy = BTreeMap::from([(
        "legacy-request".to_owned(),
        refused_approval("legacy-request"),
    )]);
    tokio::fs::write(
        &path,
        serde_json::to_vec_pretty(&legacy).expect("serialize legacy interaction history"),
    )
    .await
    .expect("write undated legacy history");
    let load_started_at = Utc::now();

    let store = InteractionHistoryStore::load(path.clone())
        .await
        .expect("load and upgrade history");
    let created_at = store
        .data
        .lock()
        .await
        .created_at
        .get("legacy-request")
        .copied()
        .expect("stamp the old entry at upgrade");
    assert!(created_at >= load_started_at && created_at <= Utc::now());
    let upgraded: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(&path)
            .await
            .expect("read upgraded history"),
    )
    .expect("parse upgraded history");
    assert!(upgraded["legacy-request"]["createdAt"].is_string());

    let exact_cutoff = created_at + Duration::days(30);
    assert_eq!(
        store
            .prune_expired(exact_cutoff, 500)
            .await
            .expect("preserve exact cutoff"),
        0
    );
    assert_eq!(
        store
            .prune_expired(exact_cutoff + Duration::nanoseconds(1), 500)
            .await
            .expect("prune one expired entry"),
        1
    );
    assert!(store.interaction("legacy-request").await.is_none());
    store
        .record_refused_approval(
            session_ref("requester"),
            Identity::Session {
                session: session_ref("approver"),
            },
            RefusedTypedApproval {
                request_id: "new-request".to_owned(),
                title: "New refusal".to_owned(),
                description: None,
                subject: None,
                options: vec![],
                reason: "fixture refusal".to_owned(),
            },
        )
        .await
        .expect("record new interaction with a timestamp");
    let persisted: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(&path)
            .await
            .expect("read timestamped history"),
    )
    .expect("parse timestamped history");
    assert!(persisted["new-request"]["createdAt"].is_string());
}
