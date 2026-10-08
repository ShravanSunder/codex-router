//! Control publishes every listed Codex session with all of its fields; a stored session with no
//! recorded model or reasoning effort carries both as `null`, as the catalog always wrote them.
use collaboration_service::{NativeControlBackend, NativeGenerationGate, ServiceIdentity};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000031";
const SESSION_FIELDS: [&str; 10] = [
    "target",
    "name",
    "title",
    "source",
    "gitBranch",
    "workingDirectory",
    "observation",
    "model",
    "reasoningEffort",
    "idleSeconds",
];

async fn stored_catalog(home: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(home.join("state_5.sqlite"))
        .create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    sqlx::raw_sql("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT NOT NULL DEFAULT '', created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER); CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO threads (id,cwd,model,reasoning_effort,name,title,preview,first_user_message,updated_at_ms,recency_at_ms,archived) VALUES ('without-model','/work/a',NULL,NULL,NULL,'No model','hello','hello',2000,2000,0), ('with-model','/work/b','gpt-5.6-sol','high','Named','With model','hi','hi',1000,1000,0)")
        .execute(&pool)
        .await?;
    pool.close().await;
    Ok(())
}

async fn send_raw(
    identity: ServiceIdentity,
    request: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let (client, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(collaboration_service::serve_control_connection(
        server, identity,
    ));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    let initialize = json!({
        "jsonrpc":"2.0","id":"initialize","method":"control/initialize",
        "params":{"version":{"major":1,"minor":0},"client":{"name":"session-shape","version":"1"}}
    });
    writer
        .write_all(format!("{initialize}\n").as_bytes())
        .await?;
    lines
        .next_line()
        .await?
        .ok_or("initialization response missing")?;
    writer.write_all(format!("{request}\n").as_bytes()).await?;
    let response = lines.next_line().await?.ok_or("list response missing")?;
    writer.shutdown().await?;
    task.await??;
    Ok(serde_json::from_str(&response)?)
}

#[tokio::test]
async fn stored_sessions_publish_absent_model_and_effort_as_null()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a stored catalog with one session lacking model and effort, one with both.
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let home = root.path().join("native-home");
    std::fs::create_dir(&home)?;
    stored_catalog(&home).await?;
    let endpoint = json!({"serviceId": SERVICE_ID, "endpointId": "codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint": endpoint,
        "label": "Stored shape fixture",
        "availability": {"state": "unprobed"},
        "channels": [{
            "kind": "nativeCodex",
            "transport": "unixWebSocket",
            "path": "absent-native.sock",
            "schemaDigest": null,
            "generation": null
        }]
    }))?;
    let identity = ServiceIdentity::new(SERVICE_ID, "00000000-0000-4000-8000-000000000032")?
        .with_endpoints(vec![description])?
        .with_native_backend(NativeControlBackend {
            endpoint: serde_json::from_value(endpoint.clone())?,
            gate: NativeGenerationGate::default(),
            codex_home: home,
        })?;
    let request = json!({
        "jsonrpc":"2.0","id":"list","method":"codex/sessionList",
        "params":{"endpoint":endpoint,"view":"stored","scope":{"kind":"any"},"source":"all","pageSize":10}
    });

    // Act.
    let response = send_raw(identity, request).await?;

    // Assert.
    let sessions = response
        .pointer("/result/sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("no sessions in {response}"))?;
    if sessions.len() != 2 {
        return Err(format!("expected two stored sessions: {response}").into());
    }
    for session in sessions {
        let fields = session
            .as_object()
            .ok_or_else(|| format!("session is not an object: {session}"))?;
        let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
        let mut expected = SESSION_FIELDS.to_vec();
        names.sort_unstable();
        expected.sort_unstable();
        if names != expected {
            return Err(
                format!("session fields differ from the published shape: {session}").into(),
            );
        }
    }
    let by_id = |id: &str| {
        sessions
            .iter()
            .find(|session| session.pointer("/target/sessionId") == Some(&json!(id)))
            .ok_or_else(|| format!("session {id} missing from {response}"))
    };
    let without = by_id("without-model")?;
    let with = by_id("with-model")?;
    if without.get("model") != Some(&Value::Null)
        || without.get("reasoningEffort") != Some(&Value::Null)
        || with.get("model") != Some(&json!("gpt-5.6-sol"))
        || with.get("reasoningEffort") != Some(&json!("high"))
    {
        return Err(format!("model and effort were not published as recorded: {response}").into());
    }
    Ok(())
}
