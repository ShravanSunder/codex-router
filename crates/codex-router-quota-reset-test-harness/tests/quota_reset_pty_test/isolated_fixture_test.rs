use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::ids::AccountId;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::account_credential_bundle_key;
use codex_router_secret_store::backend::SecretStore;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use sha2::Digest;
use sha2::Sha256;

pub(super) type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) fn ensure(condition: bool, message: &'static str) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) const ACCESS_TOKEN_CANARY: &str = "pty-access-token-canary";
const REFRESH_TOKEN_CANARY: &str = "pty-refresh-token-canary";
const FIXTURE_ROOT_MARKER_NAME: &str = ".codex-router-quota-reset-test-fixture";
const FIXTURE_ROOT_MARKER_PREFIX: &str = "codex-router-quota-reset-test-fixture:v1:";
pub(super) const FORBIDDEN_TERMINAL_CANARIES: &[&str] = &[
    ACCESS_TOKEN_CANARY,
    REFRESH_TOKEN_CANARY,
    "authorization: bearer",
    "chatgpt-account-id: routing-pty-alpha",
    "chatgpt-account-id: routing-pty-beta",
    "pty-credit-earliest",
];

pub(super) struct QuotaResetFixture {
    root: PathBuf,
    capability: String,
    state_bytes_before: Vec<u8>,
    secret_manifest_before: BTreeMap<String, String>,
}

impl QuotaResetFixture {
    pub(super) async fn create() -> TestResult<Self> {
        let sequence = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "codex-router-quota-reset-pty-{}-{}",
            std::process::id(),
            sequence
        ));
        fs::create_dir(&root)?;
        let capability = format!("pty-fixture-{}-{sequence}", std::process::id());
        fs::write(
            root.join(FIXTURE_ROOT_MARKER_NAME),
            format!("{FIXTURE_ROOT_MARKER_PREFIX}{capability}\n"),
        )?;
        let state_path = root.join("state.sqlite");
        let state = AsyncSqliteStateStore::open(&state_path).await?;
        let now = current_unix_seconds();
        for (account_id, label, generation, routing_id) in [
            ("acct_pty_alpha", "pty-alpha", 7, "routing-pty-alpha"),
            ("acct_pty_beta", "pty-beta", 11, "routing-pty-beta"),
        ] {
            let account_id = AccountId::new(account_id)?;
            state
                .upsert_account(
                    &AccountRecord::new(account_id.clone(), label, AccountStatus::Enabled)
                        .with_active_credential_generation(generation),
                )
                .await?;
            state
                .upsert_quota_snapshot(
                    &PersistedQuotaSnapshot::new(
                        account_id.clone(),
                        QuotaSnapshotSource::MockEndpoint,
                    )
                    .with_observed_unix_seconds(now)
                    .with_route_band("responses", 50)
                    .with_reset_unix_seconds(now.saturating_add(3_600))
                    .with_reset_credits_available(2)
                    .with_stale_penalty(false),
                )
                .await?;
            write_fixture_credential(&root, &account_id, generation, routing_id)?;
        }
        state.close().await?;
        let state_bytes_before = fs::read(&state_path)?;
        let secret_manifest_before = recursive_manifest(&root.join("secrets"))?;
        Ok(Self {
            root,
            capability,
            state_bytes_before,
            secret_manifest_before,
        })
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn capability(&self) -> &str {
        &self.capability
    }

    pub(super) fn assert_read_only(&self) -> TestResult<()> {
        let state_bytes_after = fs::read(self.root.join("state.sqlite"))?;
        ensure(
            self.state_bytes_before == state_bytes_after,
            "reset workflow changed fixture state bytes",
        )?;
        ensure(
            self.secret_manifest_before == recursive_manifest(&self.root.join("secrets"))?,
            "reset workflow changed fixture secret manifest",
        )
    }
}

impl Drop for QuotaResetFixture {
    fn drop(&mut self) {
        if self.root.exists() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn write_fixture_credential(
    root: &Path,
    account_id: &AccountId,
    generation: u64,
    routing_id: &str,
) -> TestResult<()> {
    let bundle = AccountCredentialBundle::imported_codex_auth(
        ACCESS_TOKEN_CANARY,
        Some(REFRESH_TOKEN_CANARY.to_owned()),
    )
    .with_expires_unix_seconds(current_unix_seconds().saturating_add(86_400))
    .with_chatgpt_account_id(routing_id)
    .to_secret_string()?;
    let store = FileSecretStore::open(root.join("secrets"))?;
    let key = account_credential_bundle_key(account_id, generation)?;
    store.write_secret(&key, &bundle)?;
    Ok(())
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn recursive_manifest(root: &Path) -> TestResult<BTreeMap<String, String>> {
    let mut manifest = BTreeMap::new();
    collect_manifest(root, root, &mut manifest)?;
    Ok(manifest)
}

fn collect_manifest(
    root: &Path,
    path: &Path,
    manifest: &mut BTreeMap<String, String>,
) -> TestResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    let relative = path.strip_prefix(root)?;
    let relative = if relative.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        relative.to_string_lossy().into_owned()
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode = 0;
    if metadata.is_file() {
        let bytes = fs::read(path)?;
        manifest.insert(
            relative,
            format!("file:{mode:o}:{}:{:x}", bytes.len(), Sha256::digest(bytes)),
        );
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(std::io::Error::other("fixture manifest refuses special files").into());
    }
    manifest.insert(relative, format!("dir:{mode:o}"));
    let mut children = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();
    for child in children {
        collect_manifest(root, &child, manifest)?;
    }
    Ok(())
}
