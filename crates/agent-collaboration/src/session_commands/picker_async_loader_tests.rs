//! The real default catalog loader runs on the caller's runtime.
use super::*;

#[tokio::test]
async fn configured_picker_loader_rejects_unsupported_source_queries_before_transport() {
    use crate::presentation::session_picker::{
        PickerSourceContext, SourceInventoryRejection, SourceInventoryRequest,
        SourceInventoryResult,
    };
    let root = tempfile::tempdir().unwrap();
    let context = CliContext::new(Vec::new()).with_current_dir(root.path().to_path_buf());
    let registry = router_connection_registry::RouterConnectionRegistry::parse(
        r#"{
        "version":1,"routers":[{"name":"Source fixture","connection":{"kind":"remote",
        "serviceId":"00000000-0000-4000-8000-000000000003","mcpUrl":"https://source.invalid/mcp"},
        "defaultRemoteCwd":"/remote/new-only"}]}"#,
    )
    .unwrap();
    let source_context = PickerSourceContext::ConfiguredHosted(registry.routers[0].clone());
    let loader = session_picker_record_loader(
        context,
        discover_repository_identity(root.path()),
        Some(root.path().join("missing-control-service")),
    );
    let base = SessionsPickerDataQuery {
        root: SessionsPickerRoot::Any,
        provider: SessionsProvider::Any,
        source: SessionsSource::All,
        sort: SessionsSort::Updated,
        search: String::new(),
        include_empty_sessions: false,
    };
    let mut cases = Vec::new();
    for scope in [SessionsPickerRoot::Cwd, SessionsPickerRoot::Repo] {
        let mut query = base.clone();
        query.root = scope;
        cases.push((query, SourceInventoryRejection::UnsupportedViewOrScope));
    }
    for provider in [
        SessionsProvider::Current,
        SessionsProvider::Id("source-provider".into()),
        SessionsProvider::ClaudeCode,
    ] {
        let mut query = base.clone();
        query.provider = provider;
        cases.push((query, SourceInventoryRejection::UnsupportedViewOrScope));
    }
    for search in ["ordinary terms", "name:alpha", "id:shared", "cwd:/caller"] {
        let mut query = base.clone();
        query.search = search.to_owned();
        cases.push((query, SourceInventoryRejection::UnsupportedQuery));
    }
    let mut whitespace_query = base.clone();
    whitespace_query.search = " \t ".to_owned();
    cases.push((
        whitespace_query,
        SourceInventoryRejection::EndpointUnqualified,
    ));
    for source in [
        SessionsSource::All,
        SessionsSource::Interactive,
        SessionsSource::Subagents,
    ] {
        for include_empty_sessions in [false, true] {
            let mut query = base.clone();
            query.source = source;
            query.include_empty_sessions = include_empty_sessions;
            cases.push((query, SourceInventoryRejection::EndpointUnqualified));
        }
    }
    for (query, expected) in cases {
        let request = SourceInventoryRequest {
            source_context: source_context.clone(),
            query,
            request_generation: 17,
        };
        let result = loader(request.clone()).await;
        assert_eq!(
            result.request(),
            &request,
            "qualification preserves the captured tuple"
        );
        let SourceInventoryResult::Rejected { reason, .. } = result else {
            panic!("no configured transport is qualified");
        };
        assert_eq!(reason, expected);
    }
    assert!(!root.path().join("missing-control-service").exists());
}

#[tokio::test]
async fn default_picker_loader_uses_the_existing_async_runtime() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex-home");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    use sqlx::Connection;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(home.join("state_5.sqlite"))
        .create_if_missing(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    // The same upstream-shaped fixture columns/indexes used by existing catalog contracts.
    for statement in [
        "CREATE TABLE threads (id TEXT PRIMARY KEY NOT NULL, rollout_path TEXT, created_at TEXT,
        updated_at TEXT, source TEXT, model_provider TEXT, cwd TEXT, name TEXT, title TEXT,
        sandbox_policy TEXT, approval_mode TEXT, tokens_used INTEGER, has_user_event INTEGER,
        archived INTEGER NOT NULL DEFAULT 0, archived_at TEXT, git_sha TEXT, git_branch TEXT,
        git_origin_url TEXT, cli_version TEXT, first_user_message TEXT NOT NULL DEFAULT '',
        agent_nickname TEXT, agent_role TEXT, memory_mode TEXT, model TEXT, reasoning_effort TEXT,
        agent_path TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, thread_source TEXT,
        preview TEXT, recency_at TEXT, recency_at_ms INTEGER)",
        "CREATE INDEX idx_threads_created_at_ms ON threads(created_at_ms DESC, id DESC)",
        "CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC)",
    ] {
        sqlx::query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO threads (id, cwd, source, thread_source, model_provider, model,
        reasoning_effort, first_user_message, created_at_ms, updated_at_ms)
        VALUES ('async-source-record', '/owned/project', 'cli', 'cli', 'codex-router',
        'gpt-6.1-sol', 'high', 'async loader fixture', 1000, 2000)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), home.display().to_string()),
        ("HOME".to_owned(), root.path().display().to_string()),
    ])
    .with_current_dir(root.path().to_path_buf());
    let identity = discover_repository_identity(root.path());
    let loader = session_picker_record_loader(context, identity, None);
    let query = SessionsPickerDataQuery {
        root: SessionsPickerRoot::Any,
        provider: SessionsProvider::Any,
        source: SessionsSource::All,
        sort: SessionsSort::Updated,
        search: String::new(),
        include_empty_sessions: false,
    };

    let records = loader(
        crate::presentation::session_picker::SourceInventoryRequest {
            source_context: crate::presentation::session_picker::PickerSourceContext::LocalCodex,
            query,
            request_generation: 7,
        },
    )
    .await
    .into_snapshot()
    .unwrap();

    assert_eq!(records.records.len(), 1);
    assert_eq!(records.records[0].session_id, "async-source-record");
    assert_eq!(records.records[0].model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(records.records[0].reasoning_effort.as_deref(), Some("high"));
    assert_eq!(
        records.runtime_coverage,
        crate::picker_runtime_status::PickerRuntimeCoverage::LocalOnly
    );
}
