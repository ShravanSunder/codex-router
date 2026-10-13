//! Native canonical scratch responses must preserve the private create contract.
use super::*;
use std::error::Error;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

type FixtureResult<TValue> = Result<TValue, Box<dyn Error + Send + Sync>>;

struct PrivateScratchFixture {
    owned_root: PathBuf,
    logical_path: PathBuf,
    canonical_path: PathBuf,
    scope: String,
}

enum ScratchAlias {
    SystemTemporaryDirectory,
    NestedDirectory,
}

impl PrivateScratchFixture {
    fn create(alias: ScratchAlias) -> FixtureResult<Self> {
        let identity = String::from(collaboration_protocol::OperationId::generate());
        let owned_root = Path::new("/tmp").join(format!("router-acp-scratch-alias-{identity}"));
        let scope = format!("session-{identity}");
        std::fs::DirBuilder::new().mode(0o700).create(&owned_root)?;
        let mut fixture = Self {
            owned_root,
            logical_path: PathBuf::new(),
            canonical_path: PathBuf::new(),
            scope,
        };
        let scratch_parent = match alias {
            ScratchAlias::SystemTemporaryDirectory => fixture.owned_root.join("scratch"),
            ScratchAlias::NestedDirectory => {
                let canonical_root = std::fs::canonicalize(&fixture.owned_root)?;
                let target = canonical_root.join("nested-target");
                std::fs::DirBuilder::new().mode(0o700).create(&target)?;
                let logical_link = canonical_root.join("nested-link");
                std::os::unix::fs::symlink(&target, &logical_link)?;
                logical_link.join("scratch")
            }
        };
        let logical_path = scratch_parent.join(&fixture.scope);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&logical_path)?;
        let canonical_path = std::fs::canonicalize(&logical_path)?;
        if logical_path == canonical_path {
            return Err("selected alias precondition".into());
        }
        if std::fs::symlink_metadata(&logical_path)?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err("private scratch precondition".into());
        }
        fixture.logical_path = logical_path;
        fixture.canonical_path = canonical_path;
        Ok(fixture)
    }
}

impl Drop for PrivateScratchFixture {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.owned_root);
    }
}

fn independent_native_write_roots(
    home: &Path,
    reported_scratch: &Path,
) -> FixtureResult<Vec<String>> {
    // Independent policy fixture: do not derive the response from emitted config.
    let cache_grants = [
        "Library/Caches",
        ".cache/uv",
        "Library/pnpm/store",
        ".local/share/pnpm/store",
        ".pnpm-store",
        ".npm/_cacache",
        ".npm/_logs",
        ".yarn/berry/cache",
        ".bun/install/cache",
        ".cargo/registry",
        ".cargo/git",
        ".cargo/.package-cache",
        ".cargo/.package-cache-mutate",
        ".cargo/.global-cache",
        ".cargo/.global-cache-journal",
        "go/pkg/mod",
        ".gradle/caches",
        ".m2/repository",
        "Library/org.swift.swiftpm/security",
        "Library/Developer/Xcode/DerivedData",
    ];
    let mut roots = vec![
        reported_scratch.to_string_lossy().into_owned(),
        "/work/project/tmp".to_owned(),
        "/work/project/docs/wip".to_owned(),
    ];
    for location in cache_grants {
        let mut path = home.to_path_buf();
        let symlinked = Path::new(location).components().any(|component| {
            path.push(component);
            std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        });
        if !symlinked {
            roots.push(home.join(location).to_string_lossy().into_owned());
        }
    }
    for temporary in std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .into_iter()
        .chain([PathBuf::from("/tmp")])
        .filter(|path| path.is_absolute())
    {
        roots.push(
            std::fs::canonicalize(temporary)?
                .to_string_lossy()
                .into_owned(),
        );
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

#[tokio::test]
async fn private_scratch_system_alias_allows_native_session_binding() -> FixtureResult<()> {
    // Arrange: one already-private validated scratch, addressed through /tmp.
    let scratch = PrivateScratchFixture::create(ScratchAlias::SystemTemporaryDirectory)?;
    let reported_scratch = scratch.canonical_path.clone();
    assert_private_scratch_binding(scratch, reported_scratch).await
}

#[tokio::test]
async fn private_scratch_nested_alias_preserves_logical_native_grant() -> FixtureResult<()> {
    // Native preserves a nested logical symlink rather than fully canonicalizing it.
    let scratch = PrivateScratchFixture::create(ScratchAlias::NestedDirectory)?;
    let reported_scratch = scratch.logical_path.clone();
    assert_private_scratch_binding(scratch, reported_scratch).await
}

async fn assert_private_scratch_binding(
    scratch: PrivateScratchFixture,
    reported_scratch: PathBuf,
) -> FixtureResult<()> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME precondition")?);
    assert!(home.is_absolute());
    let expected_roots = independent_native_write_roots(&home, &reported_scratch)?;
    let alternate_scratch = if reported_scratch == scratch.logical_path {
        &scratch.canonical_path
    } else {
        &scratch.logical_path
    }
    .to_string_lossy()
    .into_owned();
    let reported_scratch = reported_scratch.to_string_lossy().into_owned();
    let read_only_cache = home
        .join("Library/Caches/pnpm/dlx")
        .to_string_lossy()
        .into_owned();
    let mut catalog = AcpSchemaCatalog::load()?;
    let definitions: serde_json::Map<String, Value> = [
        "ThreadRead",
        "ThreadFork",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ]
    .into_iter()
    .flat_map(|name| {
        [
            (format!("{name}Params"), json!({"type":"object"})),
            (format!("{name}Response"), json!({"type":"object"})),
        ]
    })
    .collect();
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle)?);
    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )?;
    let (client, server) = tokio::net::UnixStream::pair()?;
    let connection = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let fixture = tokio::spawn(async move {
        let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let frame = server.next().await.ok_or("thread/start request")??;
        let request: Value = serde_json::from_str(frame.to_text()?)?;
        assert_eq!(
            request.get("method").and_then(Value::as_str),
            Some("thread/start")
        );
        assert_eq!(
            request
                .pointer("/params/permissions")
                .and_then(Value::as_str),
            Some("router-write-restricted")
        );
        let filesystem = request
            .pointer("/params/config")
            .and_then(|config| config.get("permissions.router-write-restricted.filesystem"))
            .and_then(Value::as_object)
            .ok_or("filesystem grants")?;
        assert!(filesystem.values().any(|access| access == "write"));
        assert_eq!(filesystem.get(&read_only_cache), Some(&json!("read")));
        let reported_granted = filesystem.get(&reported_scratch) == Some(&json!("write"));
        let alternate_granted = filesystem.get(&alternate_scratch) == Some(&json!("write"));
        // Native-side response is literal/version-shaped, never the request echo.
        server.send(Message::Text(json!({
            "id": request.get("id").ok_or("native request ID")?,
            "result": {
                "cwd":"/work/project", "model":"gpt-6-luna",
                "approvalPolicy":"on-request", "approvalsReviewer":"user",
                "activePermissionProfile":{"id":"router-write-restricted","extends":":read-only"},
                "sandbox":{"type":"workspaceWrite","writableRoots":expected_roots,
                    "networkAccess":true,"excludeTmpdirEnvVar":false,"excludeSlashTmp":false},
                "thread":{"id":"scratch-alias-thread","cwd":"/work/project",
                    "status":{"type":"idle"},"turns":[]}
            }
        }).to_string().into())).await?;
        Ok::<_, Box<dyn Error + Send + Sync>>((reported_granted, alternate_granted))
    });

    // Act: real create, scratch validation, projection and native protocol consumer.
    let result = AcpSessionBinding::create(&mut catalog, SessionSetupInputs {
        operation_id: None,
        recorder: Arc::new(AcceptingConversationRecorder),
        connection, schemas, generation,
        params: json!({"cwd":"/work/project","mcpServers":[],"_meta":{"codexRouter":{
            "model":"gpt-6-luna","effort":"medium","access":"write-restricted",
            "scratchScope":scratch.scope,"scratchPath":scratch.logical_path,
            "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001", "endpointId":"codex-local"},"sessionId":"creator"},
            "approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001", "endpointId":"codex-local"},"sessionId":"creator"}
        }}}),
        approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
    }).await;
    if result.is_err() {
        // Settle the owned peer even if setup returns before native receives a frame.
        fixture.abort();
        let _settled = fixture.await;
        return Err(format!("canonical private scratch create: {:?}", result.err()).into());
    }
    let (reported_granted, alternate_granted) = fixture.await??;

    // Assert: the consumer gets a usable binding and the canonical private grant.
    let binding = result.map_err(|error| format!("canonical private scratch create: {error:?}"))?;
    assert_eq!(binding.session_id(), "scratch-alias-thread");
    assert_eq!(
        binding.new_session_result(),
        json!({"sessionId":"scratch-alias-thread"})
    );
    assert!(
        reported_granted,
        "outgoing source-qualified private scratch grant"
    );
    assert!(!alternate_granted, "no duplicate alternate scratch grant");
    let owned_root = scratch.owned_root.clone();
    drop(binding);
    drop(scratch);
    if owned_root.try_exists()? {
        return Err("owned scratch cleanup incomplete".into());
    }
    Ok(())
}
