use super::*;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) fn local_auth_gate() -> ProxyLocalAuthGate {
    let current =
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1));
    ProxyLocalAuthGate::new(LocalRouterAuth::new(current, Vec::new()))
}

pub(super) fn account_id(value: &str) -> codex_router_core::ids::AccountId {
    match codex_router_core::ids::AccountId::new(value) {
        Ok(account_id) => account_id,
        Err(error) => panic!("account id should parse: {error}"),
    }
}

pub(super) fn test_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(super) fn selector_reset_seconds(limit_window_seconds: u64) -> u64 {
    test_unix_seconds().saturating_add(limit_window_seconds)
}

pub(super) struct ProxyTestTempDir {
    path: PathBuf,
}

impl ProxyTestTempDir {
    pub(super) fn new(name: &str) -> Self {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "codex-router-proxy-{name}-{}-{unique}",
            std::process::id()
        ));
        if path.exists() {
            remove_dir_all(&path);
        }
        if let Err(error) = fs::create_dir(&path) {
            panic!(
                "failed to create test directory {}: {error}",
                path.display()
            );
        }

        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProxyTestTempDir {
    fn drop(&mut self) {
        if self.path.exists() {
            remove_dir_all(&self.path);
        }
    }
}

pub(super) fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected Ok, got error: {error}"),
    }
}

fn remove_dir_all(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path) {
        panic!(
            "failed to remove test directory {}: {error}",
            path.display()
        );
    }
}
