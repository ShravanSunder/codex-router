use super::*;

pub(super) fn test_async_runtime() -> tokio::runtime::Runtime {
    must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    )
}

pub(super) struct TestRoot {
    pub(super) path: PathBuf,
}

impl TestRoot {
    pub(super) fn new(name: &str) -> Self {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "codex-router-cli-token-{name}-{}-{counter}",
            std::process::id()
        ));
        if path.exists() {
            remove_dir_all(&path);
        }

        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

pub(super) fn default_router_root_for_test() -> PathBuf {
    let Some(home) = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        panic!("HOME must be set for default router root tests");
    };

    home.join(".codex-router")
}

pub(super) fn default_router_secret_root_for_test() -> PathBuf {
    default_router_root_for_test().join("secrets")
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        if self.path.exists() {
            remove_dir_all(&self.path);
        }
    }
}

pub(super) struct FailingSecretStore {
    pub(super) write_attempts: AtomicUsize,
}

impl FailingSecretStore {
    pub(super) fn new() -> Self {
        Self {
            write_attempts: AtomicUsize::new(0),
        }
    }

    pub(super) fn write_attempts(&self) -> usize {
        self.write_attempts.load(Ordering::SeqCst)
    }
}

impl SecretStore for FailingSecretStore {
    fn write_secret(
        &self,
        _key: &SecretKey,
        _secret: &SecretString,
    ) -> Result<(), SecretStoreError> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);

        Err(SecretStoreError::Filesystem {
            path: PathBuf::from("injected-secret-store-failure"),
            source: std::io::Error::other("injected secret-store failure"),
        })
    }

    fn read_secret(&self, _key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        Err(SecretStoreError::Filesystem {
            path: PathBuf::from("injected-secret-store-failure"),
            source: std::io::Error::other("injected secret-store failure"),
        })
    }
}

pub(super) fn assert_router_profile_contract(output: &str, port: u16) {
    assert!(!output.contains("[profiles.codex-router]\n"));
    assert!(output.contains("model_provider = \"codex-router\"\n"));
    assert!(output.contains("[model_providers.codex-router]\n"));
    assert!(output.contains("name = \"codex-router\"\n"));
    assert!(output.contains(format!("base_url = \"http://127.0.0.1:{port}/v1\"\n").as_str()));
    assert!(output.contains("wire_api = \"responses\"\n"));
    assert!(output.contains("requires_openai_auth = false\n"));
    assert!(output.contains("supports_websockets = true\n"));
    assert!(!output.contains("env_key"));
    assert!(!output.contains("env_http_headers"));
    assert!(!output.contains("sk-"));
    assert!(!output.contains("oauth"));
}

pub(super) fn git_diff_text(workspace_root: &Path, args: &[&str]) -> String {
    let output = match ProcessCommand::new("git")
        .args(args)
        .current_dir(workspace_root)
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return String::new(),
    };
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(super) fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected Ok, got error: {error}"),
    }
}

pub(super) fn strip_ansi_sequences(input: &str) -> String {
    let mut output = String::new();
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\x1b' && characters.peek() == Some(&'[') {
            characters.next();
            for sequence_character in characters.by_ref() {
                if sequence_character.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

pub(super) fn lock_test_mutex<'a, T>(mutex: &'a Mutex<T>, label: &str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(error) => panic!("{label} lock should be available: {error}"),
    }
}

pub(super) fn must_err<T, E: std::fmt::Display>(result: Result<T, E>) -> E {
    match result {
        Ok(_) => panic!("expected error, got Ok"),
        Err(error) => error,
    }
}

pub(super) fn fake_id_token_with_chatgpt_account_id(account_id: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let payload = serde_json::json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id
        }
    });
    format!(
        "header.{}.signature",
        URL_SAFE_NO_PAD.encode(payload.to_string())
    )
}

pub(super) fn remove_dir_all(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path) {
        panic!(
            "failed to remove test directory {}: {error}",
            path.display()
        );
    }
}

pub(super) struct CliRunOutput {
    pub(super) stdout: String,
    pub(super) stderr: String,
}

pub(super) fn run_cli<const ARGUMENT_COUNT: usize>(
    args: [&str; ARGUMENT_COUNT],
    context: CliContext,
) -> CliRunOutput {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let arguments = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if matches!(
        must_ok(CliCommand::parse(arguments.clone())),
        CliCommand::Quota(_)
    ) {
        must_ok(test_async_runtime().block_on(run_with_io_async(
            arguments,
            &context,
            &mut stdout,
            &mut stderr,
        )));
    } else {
        must_ok(run_with_io(arguments, &context, &mut stdout, &mut stderr));
    }

    CliRunOutput {
        stdout: must_ok(String::from_utf8(stdout)),
        stderr: must_ok(String::from_utf8(stderr)),
    }
}

pub(super) fn ensure_async_state_schema(router_root: &Path) {
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    must_ok(runtime.block_on(state.close()));
}

pub(super) fn path_to_str(path: &Path) -> &str {
    match path.to_str() {
        Some(path) => path,
        None => panic!("test path must be UTF-8"),
    }
}

pub(super) fn account_id(value: &str) -> AccountId {
    match AccountId::new(value) {
        Ok(account_id) => account_id,
        Err(error) => panic!("account id should parse: {error}"),
    }
}
