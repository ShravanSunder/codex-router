use crate::proxy_role_test_fixtures::*;
use crate::*;
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener};
use codex_router_keeper_protocol::{ChildComponent, ChildDegradation, PrepareFailure, PrepareMode};
use codex_router_state::schema_preparation::AccountSchemaPreparation;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn fresh_creators_are_stable_and_state_waits_for_activation() {
    let root = tempfile::tempdir().expect("isolated root");
    let first = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("Fresh bootstrap");
    assert!(matches!(
        first.state_schema(),
        ProxyPreparedStateSchema::Uninitialized
    ));
    assert!(!root.path().join("state.sqlite").exists());
    let before = snapshot(root.path());
    let second = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("second Fresh open");
    assert_eq!(
        snapshot(root.path()),
        before,
        "stable token/key/id/marker/affinity bytes and modes"
    );
    assert!(!root.path().join("state.sqlite").exists());
    drop(second);
    drop(first);
}

#[tokio::test]
async fn replacement_current_keeps_snapshot_and_old_hyper_usable() {
    let root = tempfile::tempdir().expect("isolated root");
    let mut old = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("Fresh")
        .activate()
        .await
        .expect("old actual runtime");
    let reply = http(old.local_addr(), HEALTH_REQUEST).await;
    assert!(reply.starts_with(b"HTTP/1.1 200"));
    // Stop clocks/maintenance through actual shutdown only after evaluating the candidate; the old keeps serving.
    let before = snapshot(root.path());
    let candidate = prepare(
        root.path(),
        PrepareMode::Replacement {
            active_degraded: Vec::new(),
        },
    )
    .await
    .expect("replacement current");
    assert!(matches!(
        candidate.state_schema(),
        ProxyPreparedStateSchema::Existing(AccountSchemaPreparation::Current)
    ));
    let after_prepare = snapshot(root.path());
    let prepare_differences =
        describe_snapshot_differences(&before, &after_prepare, "after_prepare");
    drop(candidate);
    let after_drop = snapshot(root.path());
    let drop_differences =
        describe_snapshot_differences(&before, &after_drop, "after_candidate_drop");
    let old_reply = http(old.local_addr(), HEALTH_REQUEST).await;
    eprintln!(
        "live-old literal HTTP before/after candidate: after_status_200={}",
        old_reply.starts_with(b"HTTP/1.1 200")
    );
    old.shutdown().await.expect("joined old owners");
    assert!(old_reply.starts_with(b"HTTP/1.1 200"));
    assert!(
        after_prepare == before,
        "complete root changed during Prepare: {prepare_differences:?}"
    );
    assert!(
        after_drop == before,
        "complete root changed after candidate drop: {drop_differences:?}"
    );
}

#[tokio::test]
async fn replacement_native_prefix_is_unchanged_then_activated_and_preserves_data() {
    let root = tempfile::tempdir().expect("isolated prefix root");
    drop(
        prepare(root.path(), PrepareMode::Fresh)
            .await
            .expect("bootstrap secrets only"),
    );
    prefix_database(&root.path().join("state.sqlite")).await;
    let before = snapshot(root.path());
    let prepared = prepare(
        root.path(),
        PrepareMode::Replacement {
            active_degraded: Vec::new(),
        },
    )
    .await
    .expect("prefix Prepare");
    let versions = match prepared.state_schema() {
        ProxyPreparedStateSchema::Existing(AccountSchemaPreparation::Pending { migrations }) => {
            migrations
                .as_slice()
                .iter()
                .map(|version| version.get())
                .collect::<Vec<_>>()
        }
        _ => panic!("actual Pending prefix"),
    };
    assert_eq!(
        versions,
        vec![202610020001],
        "independent literal latest migration"
    );
    assert_eq!(
        snapshot(root.path()),
        before,
        "Prepare preserves full file bytes/modes/schema/history"
    );
    let began = Instant::now();
    let mut active = prepared
        .activate()
        .await
        .expect("migration-first activation");
    let elapsed = began.elapsed();
    eprintln!(
        "migration-bearing activation elapsed_us={} pending={versions:?}",
        elapsed.as_micros()
    );
    assert!(
        elapsed <= Duration::from_millis(500),
        "ACTIVATE_DEADLINE overrun: {elapsed:?}"
    );
    let state = codex_router_state::sqlite::AsyncSqliteStateStore::open_read_only(
        &root.path().join("state.sqlite"),
    )
    .await
    .expect("current schema only after Activate");
    let account = state
        .load_account(&codex_router_core::ids::AccountId::new("role-prefix").expect("account id"))
        .await
        .expect("real account query")
        .expect("preserved account");
    assert_eq!(account.label(), "preserved");
    assert_eq!(account.active_credential_generation(), Some(41));
    state.close().await.expect("fixture read store closes");
    assert!(
        http(active.local_addr(), HEALTH_REQUEST)
            .await
            .starts_with(b"HTTP/1.1 200")
    );
    active.shutdown().await.expect("owned runtime joins");
}

#[tokio::test]
async fn supplied_duplicate_serves_only_after_activate_and_parent_listener_survives() {
    let root = tempfile::tempdir().expect("isolated listener root");
    let supplier = held_listener().await;
    let address = supplier.tcp_address().expect("actual assigned port");
    let gate = DescriptorGate::global();
    let duplicate = gate
        .duplicate(supplier.as_fd())
        .await
        .expect("gated duplicate");
    let grant = OwnedListener::from_tcp_owned(duplicate, address, gate)
        .await
        .expect("real validated TCP association");
    let prepared = ProxyRoleRuntime::prepare(
        role_config(root.path(), address),
        PrepareMode::Fresh,
        grant,
        gate,
    )
    .await
    .expect("supplied Prepare");
    drop(supplier);
    let mut client = tokio::net::TcpStream::connect(address)
        .await
        .expect("same port queues before Activate");
    client
        .write_all(HEALTH_REQUEST)
        .await
        .expect("literal queued request");
    let mut prefix = [0u8; 12];
    assert!(
        tokio::time::timeout(Duration::from_millis(30), client.read_exact(&mut prefix))
            .await
            .is_err(),
        "candidate never accepts while Prepared"
    );
    let mut active = prepared.activate().await.expect("actual Hyper Activate");
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut prefix))
        .await
        .expect("first reply after activation")
        .expect("literal prefix");
    assert_eq!(&prefix, b"HTTP/1.1 200");
    drop(client);
    active.shutdown().await.expect("actual joined owners");
}

#[tokio::test]
async fn wrong_listener_association_is_refused_without_state_creation() {
    let root = tempfile::tempdir().expect("isolated root");
    let listener = held_listener().await;
    let address = listener.tcp_address().expect("TCP");
    let other = held_listener().await;
    let other_address = other.tcp_address().expect("different occupied port");
    assert_ne!(address, other_address);
    let result = ProxyRoleRuntime::prepare(
        role_config(root.path(), other_address),
        PrepareMode::Fresh,
        listener,
        DescriptorGate::global(),
    )
    .await;
    assert!(matches!(
        result,
        Err(ProxyPreparationError::Core(
            codex_router_proxy::server::LoopbackRouterRuntimeError::ListenerAssociation
        ))
    ));
    assert!(!root.path().join("state.sqlite").exists());
}

#[tokio::test]
async fn missing_replacement_root_never_falls_back_to_creators() {
    let root = tempfile::tempdir().expect("isolated root");
    let before = snapshot(root.path());
    let error = prepare(
        root.path(),
        PrepareMode::Replacement {
            active_degraded: Vec::new(),
        },
    )
    .await
    .err()
    .expect("missing existing prerequisites refuse");
    assert_eq!(
        error.prepare_failure(),
        PrepareFailure::SecretStoreUnavailable
    );
    assert_eq!(snapshot(root.path()), before);
}

#[tokio::test]
async fn invalid_degradation_parameters_have_no_filesystem_effect() {
    let root = tempfile::tempdir().expect("isolated root");
    let before = snapshot(root.path());
    let error = prepare(
        root.path(),
        PrepareMode::Replacement {
            active_degraded: vec![(ChildComponent::Board, ChildDegradation::StoreUnavailable)],
        },
    )
    .await
    .err()
    .expect("wrong role parameters");
    assert_eq!(error.prepare_failure(), PrepareFailure::FrameInvalid);
    assert_eq!(snapshot(root.path()), before);
}

#[tokio::test]
async fn changed_history_is_revalidated_at_activate_without_downgrade() {
    use sqlx::Connection;
    let root = tempfile::tempdir().expect("isolated root");
    drop(
        prepare(root.path(), PrepareMode::Fresh)
            .await
            .expect("secret bootstrap"),
    );
    prefix_database(&root.path().join("state.sqlite")).await;
    let prepared = prepare(
        root.path(),
        PrepareMode::Replacement {
            active_degraded: Vec::new(),
        },
    )
    .await
    .expect("valid prefix");
    let mut db = sqlx::SqliteConnection::connect(&format!(
        "sqlite:{}",
        root.path().join("state.sqlite").display()
    ))
    .await
    .expect("fixture writer");
    sqlx::query("UPDATE _sqlx_migrations SET checksum=X'00'")
        .execute(&mut db)
        .await
        .expect("actual damaged history");
    db.close().await.expect("writer closes");
    let before = snapshot(root.path());
    let error = prepared
        .activate()
        .await
        .err()
        .expect("activation refuses changed history");
    assert!(matches!(error,ProxyActivationError::Core(codex_router_proxy::server::LoopbackRouterRuntimeError::SchemaPreparation(codex_router_state::schema_preparation::StateSchemaPreparationError::ChecksumMismatch))));
    assert_eq!(snapshot(root.path()), before);
}

#[tokio::test]
async fn replacement_current_freezes_entire_state_and_secret_tree() {
    let root = tempfile::tempdir().expect("isolated current root");
    let mut old = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("old role preparation")
        .activate()
        .await
        .expect("actual live old role");
    assert!(
        http(old.local_addr(), HEALTH_REQUEST)
            .await
            .starts_with(b"HTTP/1.1 200")
    );
    let before = snapshot(root.path());
    let candidate = prepare(
        root.path(),
        PrepareMode::Replacement {
            active_degraded: Vec::new(),
        },
    )
    .await
    .expect("Current Prepare");
    assert!(matches!(
        candidate.state_schema(),
        ProxyPreparedStateSchema::Existing(AccountSchemaPreparation::Current)
    ));
    let after = snapshot(root.path());
    let changed = before
        .keys()
        .chain(after.keys())
        .filter(|key| before.get(*key) != after.get(*key))
        .collect::<std::collections::BTreeSet<_>>();
    for path in &changed {
        eprintln!(
            "Current Prepare snapshot difference path={} before_len={:?} after_len={:?}",
            path.display(),
            before
                .get(*path)
                .map(|(_, bytes)| bytes.as_ref().map(Vec::len)),
            after
                .get(*path)
                .map(|(_, bytes)| bytes.as_ref().map(Vec::len))
        );
    }
    drop(candidate);
    let after_drop = snapshot(root.path());
    old.shutdown()
        .await
        .expect("old owners join before assertions");
    assert!(
        after == before,
        "full Current state snapshot changed at {changed:?}"
    );
    assert!(
        after_drop == before,
        "candidate drop must preserve snapshot"
    );
}

#[tokio::test]
async fn malformed_existing_secret_prerequisites_never_repair_or_publish() {
    for (name, contents) in [
        ("format-v2.marker", b"invalid".as_slice()),
        ("store-id", b"invalid".as_slice()),
        (
            "local_router_token_generation.secret",
            b"not-a-generation".as_slice(),
        ),
        (
            "router_affinity_hash_secret.v1.secret",
            b"invalid".as_slice(),
        ),
    ] {
        let root = tempfile::tempdir().expect("isolated prerequisite root");
        drop(
            prepare(root.path(), PrepareMode::Fresh)
                .await
                .expect("actual Fresh prerequisites"),
        );
        let state = codex_router_state::sqlite::AsyncSqliteStateStore::open(
            &root.path().join("state.sqlite"),
        )
        .await
        .expect("fixture current state");
        state.close().await.expect("state closes");
        let path = root.path().join("secrets").join(name);
        assert!(path.exists(), "literal fixture prerequisite exists: {name}");
        std::fs::write(&path, contents).expect("controlled corruption");
        let before = snapshot(root.path());
        let error = prepare(
            root.path(),
            PrepareMode::Replacement {
                active_degraded: Vec::new(),
            },
        )
        .await
        .err()
        .expect("invalid existing prerequisite must refuse");
        assert_eq!(
            error.prepare_failure(),
            PrepareFailure::SecretStoreUnavailable,
            "exact prerequisite refusal: {name}"
        );
        assert_eq!(snapshot(root.path()), before);
    }
}

#[tokio::test]
async fn invalid_native_history_refuses_prepare_without_file_changes() {
    use sqlx::Connection;
    for (sql, expected) in [
        (
            "UPDATE _sqlx_migrations SET success=0",
            PrepareFailure::StoreMigrationHistoryInvalid {
                store: codex_router_keeper_protocol::StoreKind::RouterState,
                reason: codex_router_keeper_protocol::MigrationHistoryDefect::DirtyMigration,
            },
        ),
        (
            "UPDATE _sqlx_migrations SET checksum=X'00'",
            PrepareFailure::StoreMigrationHistoryInvalid {
                store: codex_router_keeper_protocol::StoreKind::RouterState,
                reason: codex_router_keeper_protocol::MigrationHistoryDefect::ChecksumMismatch,
            },
        ),
        (
            "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(209901010001,'future',1,X'00',1)",
            PrepareFailure::StoreSchemaNewerThanImage {
                store: codex_router_keeper_protocol::StoreKind::RouterState,
            },
        ),
    ] {
        let root = tempfile::tempdir().expect("isolated invalid history root");
        drop(
            prepare(root.path(), PrepareMode::Fresh)
                .await
                .expect("actual secret bootstrap"),
        );
        prefix_database(&root.path().join("state.sqlite")).await;
        let mut db = sqlx::SqliteConnection::connect(&format!(
            "sqlite:{}",
            root.path().join("state.sqlite").display()
        ))
        .await
        .expect("fixture history writer");
        sqlx::query(sql)
            .execute(&mut db)
            .await
            .expect("controlled history defect");
        db.close().await.expect("fixture writer closes");
        let before = snapshot(root.path());
        let error = prepare(
            root.path(),
            PrepareMode::Replacement {
                active_degraded: Vec::new(),
            },
        )
        .await
        .err()
        .expect("real damaged history refuses");
        assert_eq!(error.prepare_failure(), expected);
        assert_eq!(snapshot(root.path()), before);
    }
}

#[tokio::test]
async fn unlimited_role_reports_malformed_client_and_keeps_accepting() {
    let root = tempfile::tempdir().expect("isolated malformed-client root");
    let mut role = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("Fresh")
        .activate()
        .await
        .expect("actual role");
    let mut malformed = tokio::net::TcpStream::connect(role.local_addr())
        .await
        .expect("malformed client");
    malformed
        .write_all(b"not-http\r\n\r\n")
        .await
        .expect("malformed literal bytes");
    malformed.shutdown().await.expect("client half close");
    let mut refused = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), malformed.read_to_end(&mut refused))
        .await
        .expect("connection-level refusal finishes")
        .expect("refusal read");
    assert!(
        http(role.local_addr(), HEALTH_REQUEST)
            .await
            .starts_with(b"HTTP/1.1 200"),
        "one malformed request never stops the unlimited role"
    );
    role.shutdown().await.expect("all acquired owners join");
}

#[tokio::test]
async fn dropped_shutdown_wait_retains_actual_http_and_worker_owners_for_resumption() {
    use std::future::Future;
    use std::task::Poll;
    let root = tempfile::tempdir().expect("isolated cancellation root");
    let mut role = prepare(root.path(), PrepareMode::Fresh)
        .await
        .expect("Fresh")
        .activate()
        .await
        .expect("actual role");
    let mut client = tokio::net::TcpStream::connect(role.local_addr())
        .await
        .expect("actual held HTTP connection");
    client
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("literal keepalive request");
    let mut prefix = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut prefix))
        .await
        .expect("actual first response")
        .expect("response prefix");
    assert_eq!(&prefix, b"HTTP/1.1 200");
    let mut shutdown = Box::pin(role.shutdown());
    std::future::poll_fn(|context| {
        assert!(
            matches!(shutdown.as_mut().poll(context), Poll::Pending),
            "actual acquired work remains owned through a pending shutdown wait"
        );
        Poll::Ready(())
    })
    .await;
    drop(shutdown);
    tokio::time::timeout(Duration::from_secs(2), role.shutdown())
        .await
        .expect("same-owner resumed shutdown bounded fixture")
        .expect("real joined cleanup");
    let mut remaining = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut remaining))
        .await
        .expect("original peer reaches EOF")
        .expect("EOF after actual owned connection joins");
    drop(role);
}
