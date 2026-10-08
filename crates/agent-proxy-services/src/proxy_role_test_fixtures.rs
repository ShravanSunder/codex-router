use crate::*;
use codex_router_descriptor_boundary::{DescriptorGate, OwnedListener};
use codex_router_keeper_protocol::PrepareMode;
use codex_router_proxy::{
    server::{LoopbackBindAddress, LoopbackRouterRuntimeConfig},
    upstream::UpstreamEndpoint,
};
use sqlx::{Connection, SqliteConnection, migrate::Migrator, sqlite::SqliteConnectOptions};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(super) fn role_config(root: &Path, address: std::net::SocketAddr) -> ProxyRoleConfig {
    ProxyRoleConfig {
        core: LoopbackRouterRuntimeConfig::new_tokenless(
            LoopbackBindAddress::new("127.0.0.1", address.port()).expect("loopback fixture"),
            UpstreamEndpoint::new("http://127.0.0.1:1/v1").expect("upstream fixture"),
            root.join("state.sqlite"),
            root.join("secrets"),
        ),
        local_token: ProxyLocalTokenPolicy::Optional,
        quota_refresh: ProxyQuotaRefreshPolicy::Disabled,
        quota_refresh_interval: Duration::from_secs(180),
        max_connections: usize::MAX,
    }
}
pub(super) async fn held_listener() -> OwnedListener {
    OwnedListener::bind_tcp(
        "127.0.0.1:0".parse().expect("loopback"),
        DescriptorGate::global(),
    )
    .await
    .expect("owned fixture listener")
}
pub(super) async fn prepare(
    root: &Path,
    mode: PrepareMode,
) -> Result<PreparedProxyRoleRuntime, ProxyPreparationError> {
    let listener = held_listener().await;
    let config = role_config(root, listener.tcp_address().expect("fixture TCP"));
    ProxyRoleRuntime::prepare(config, mode, listener, DescriptorGate::global()).await
}
pub(super) fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
    fn collect(root: &Path, dir: &Path, map: &mut BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>) {
        if !dir.exists() {
            return;
        }
        for entry in std::fs::read_dir(dir).expect("fixture snapshot directory") {
            let path = entry.expect("snapshot entry").path();
            let metadata = std::fs::symlink_metadata(&path).expect("snapshot metadata");
            let relative = path
                .strip_prefix(root)
                .expect("fixture descendant")
                .to_path_buf();
            if metadata.is_dir() {
                map.insert(relative, (metadata.permissions().mode(), None));
                collect(root, &path, map);
            } else {
                assert!(metadata.is_file(), "no links in fixture state root");
                map.insert(
                    relative,
                    (
                        metadata.permissions().mode(),
                        Some(std::fs::read(&path).expect("snapshot bytes")),
                    ),
                );
            }
        }
    }
    let mut map = BTreeMap::new();
    collect(root, root, &mut map);
    map
}
pub(super) async fn prefix_database(path: &Path) {
    let mut migrator = Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../codex-router-state/migrations"),
    )
    .await
    .expect("actual state migrations");
    let migrations = migrator.iter().cloned().collect::<Vec<_>>();
    assert!(migrations.len() > 1);
    migrator.migrations = std::borrow::Cow::Owned(
        migrations
            .into_iter()
            .rev()
            .skip(1)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
    );
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
    .expect("prefix DB");
    migrator
        .run(&mut connection)
        .await
        .expect("real prefix migrations");
    sqlx::query("INSERT INTO accounts (account_id,label,status,active_credential_generation,provider) VALUES ('role-prefix','preserved','disabled',41,'openai')").execute(&mut connection).await.expect("representative existing account");
    connection.close().await.expect("prefix fixture closes");
}
pub(super) async fn http(address: std::net::SocketAddr, request: &[u8]) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("actual TCP connection");
    stream
        .write_all(request)
        .await
        .expect("literal HTTP request");
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut bytes))
        .await
        .expect("bounded real reply")
        .expect("reply bytes");
    bytes
}
pub(super) const HEALTH_REQUEST: &[u8] =
    b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

pub(super) fn describe_snapshot_differences(
    before: &BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>,
    after: &BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>,
    phase: &str,
) -> Vec<PathBuf> {
    let paths = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for path in &paths {
        let old = before.get(path);
        let new = after.get(path);
        let positions = match (
            old.and_then(|(_, bytes)| bytes.as_ref()),
            new.and_then(|(_, bytes)| bytes.as_ref()),
        ) {
            (Some(old), Some(new)) => old
                .iter()
                .zip(new)
                .enumerate()
                .filter(|(_, pair)| pair.0 != pair.1)
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        eprintln!(
            "live-old snapshot phase={phase} path={} before_mode={:?} after_mode={:?} before_len={:?} after_len={:?} differing_byte_positions={positions:?}",
            path.display(),
            old.map(|(mode, _)| format!("{mode:o}")),
            new.map(|(mode, _)| format!("{mode:o}")),
            old.and_then(|(_, bytes)| bytes.as_ref().map(Vec::len)),
            new.and_then(|(_, bytes)| bytes.as_ref().map(Vec::len))
        );
    }
    paths.into_iter().collect()
}
