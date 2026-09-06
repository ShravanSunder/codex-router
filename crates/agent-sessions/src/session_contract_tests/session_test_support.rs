use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn must_ok<TValue, TError: std::fmt::Display>(result: Result<TValue, TError>) -> TValue {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected Ok, got error: {error}"),
    }
}
pub(super) fn must_err<TValue, TError: std::fmt::Display>(
    result: Result<TValue, TError>,
) -> TError {
    match result {
        Ok(_) => panic!("expected error, got Ok"),
        Err(error) => error,
    }
}
pub(super) fn test_async_runtime() -> tokio::runtime::Runtime {
    must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    )
}
pub(super) struct TestRoot {
    path: PathBuf,
}
impl TestRoot {
    pub(super) fn new(name: &str) -> Self {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "agent-sessions-{name}-{}-{counter}",
            std::process::id()
        ));
        Self { path }
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}
pub(super) fn parse_session_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<crate::sessions::SessionsCommand, String> {
    crate::sessions::SessionsCommand::parse(arguments.into_iter().collect())
}
pub(super) fn run_session_cli(
    arguments: impl IntoIterator<Item = OsString>,
    context: &CliContext,
    stdout: &mut impl std::io::Write,
    _stderr: &mut impl std::io::Write,
) -> Result<(), String> {
    crate::sessions::run_sessions_command(stdout, parse_session_arguments(arguments)?, context)
        .map_err(|error| error.to_string())
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
    must_ok(run_session_cli(
        args.into_iter().map(OsString::from),
        &context,
        &mut stdout,
        &mut stderr,
    ));
    CliRunOutput {
        stdout: must_ok(String::from_utf8(stdout)),
        stderr: must_ok(String::from_utf8(stderr)),
    }
}
pub(super) fn assert_session_ids(stdout: &str, expected_session_ids: &[&str]) {
    let sessions: serde_json::Value = must_ok(serde_json::from_str(stdout));
    let sessions = match sessions.as_array() {
        Some(sessions) => sessions,
        None => panic!("sessions output should be array"),
    };
    let actual_session_ids = sessions
        .iter()
        .map(|session| match session["session_id"].as_str() {
            Some(session_id) => session_id,
            None => panic!("session_id should be string in {session}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(actual_session_ids, expected_session_ids);
}

pub(super) struct CodexStateThreadFixture {
    pub(super) id: String,
    pub(super) cwd: PathBuf,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) source: String,
    pub(super) thread_source: Option<String>,
    pub(super) git_branch: String,
    pub(super) git_origin_url: Option<String>,
    pub(super) name: Option<String>,
    pub(super) title: Option<String>,
    pub(super) preview: Option<String>,
    pub(super) first_user_message: Option<String>,
    pub(super) recency_at_ms: i64,
}

impl CodexStateThreadFixture {
    pub(super) fn new(
        id: &str,
        cwd: &Path,
        provider: &str,
        source: &str,
        thread_source: &str,
        git_branch: &str,
        recency_at_ms: i64,
    ) -> Self {
        Self {
            id: id.to_owned(),
            cwd: cwd.to_path_buf(),
            provider: provider.to_owned(),
            model: "gpt-5.4-mini".to_owned(),
            source: source.to_owned(),
            thread_source: Some(thread_source.to_owned()),
            git_branch: git_branch.to_owned(),
            git_origin_url: None,
            name: None,
            title: None,
            preview: None,
            first_user_message: None,
            recency_at_ms,
        }
    }

    pub(super) fn with_thread_source(mut self, thread_source: Option<&str>) -> Self {
        self.thread_source = thread_source.map(str::to_owned);
        self
    }

    pub(super) fn with_git_origin(mut self, git_origin_url: &str) -> Self {
        self.git_origin_url = Some(git_origin_url.to_owned());
        self
    }

    pub(super) fn with_name(mut self, name: &str) -> Self {
        self.name = Some(name.to_owned());
        self
    }

    pub(super) fn with_search_fields(
        mut self,
        title: &str,
        preview: &str,
        first_user_message: &str,
    ) -> Self {
        self.title = Some(title.to_owned());
        self.preview = Some(preview.to_owned());
        self.first_user_message = Some(first_user_message.to_owned());
        self
    }
}

#[derive(Default)]
pub(super) struct FakeSessionsCommandRunner {
    pub(super) new_codex_args: Vec<Vec<OsString>>,
    pub(super) resumed_session_ids: Vec<String>,
    pub(super) resume_codex_args: Vec<Vec<OsString>>,
    pub(super) forked_session_ids: Vec<String>,
    pub(super) fork_codex_args: Vec<Vec<OsString>>,
}

impl crate::sessions::SessionsCommandRunner for FakeSessionsCommandRunner {
    fn run_codex_new(
        &mut self,
        codex_args: &[OsString],
    ) -> Result<(), crate::sessions::SessionsCommandError> {
        self.new_codex_args.push(codex_args.to_vec());
        Ok(())
    }

    fn run_codex_resume(
        &mut self,
        codex_args: &[OsString],
        session_id: &str,
    ) -> Result<(), crate::sessions::SessionsCommandError> {
        self.resume_codex_args.push(codex_args.to_vec());
        self.resumed_session_ids.push(session_id.to_owned());
        Ok(())
    }

    fn run_codex_fork(
        &mut self,
        codex_args: &[OsString],
        session_id: &str,
    ) -> Result<(), crate::sessions::SessionsCommandError> {
        self.fork_codex_args.push(codex_args.to_vec());
        self.forked_session_ids.push(session_id.to_owned());
        Ok(())
    }
}

pub(super) struct FakeSessionsPicker {
    pub(super) selected_outcome: crate::presentation::session_picker::SessionsPickerOutcome,
    pub(super) offered_session_ids: Vec<String>,
    pub(super) offered_labels: Vec<String>,
    pub(super) new_session_args_display: Option<String>,
    pub(super) loader_queries: Vec<crate::presentation::session_picker::SessionsPickerDataQuery>,
    pub(super) loaded_session_ids: Vec<Vec<String>>,
}

impl FakeSessionsPicker {
    pub(super) fn new(selected_session_id: &str) -> Self {
        Self {
            selected_outcome:
                crate::presentation::session_picker::SessionsPickerOutcome::ResumeSession(
                    selected_session_id.to_owned(),
                ),
            offered_session_ids: Vec::new(),
            offered_labels: Vec::new(),
            new_session_args_display: None,
            loader_queries: Vec::new(),
            loaded_session_ids: Vec::new(),
        }
    }

    pub(super) fn new_start_new() -> Self {
        Self {
            selected_outcome:
                crate::presentation::session_picker::SessionsPickerOutcome::StartNewSession,
            offered_session_ids: Vec::new(),
            offered_labels: Vec::new(),
            new_session_args_display: None,
            loader_queries: Vec::new(),
            loaded_session_ids: Vec::new(),
        }
    }

    pub(super) fn new_fork(session_id: &str) -> Self {
        Self {
            selected_outcome:
                crate::presentation::session_picker::SessionsPickerOutcome::ForkSession(
                    session_id.to_owned(),
                ),
            offered_session_ids: Vec::new(),
            offered_labels: Vec::new(),
            new_session_args_display: None,
            loader_queries: Vec::new(),
            loaded_session_ids: Vec::new(),
        }
    }

    pub(super) fn with_loader_query(
        mut self,
        query: crate::presentation::session_picker::SessionsPickerDataQuery,
    ) -> Self {
        self.loader_queries.push(query);
        self
    }
}

impl crate::sessions::SessionsPicker for FakeSessionsPicker {
    fn select_session(
        &mut self,
        request: crate::presentation::session_picker::SessionsPickerRequest,
        record_loader: Option<crate::presentation::session_picker::SessionsPickerRecordLoader>,
    ) -> Result<
        Option<crate::presentation::session_picker::SessionsPickerOutcome>,
        crate::sessions::SessionsCommandError,
    > {
        self.new_session_args_display = Some(request.new_session_args_display.clone());
        self.offered_session_ids = request
            .records
            .iter()
            .map(|record| record.session_id.to_owned())
            .collect();
        self.offered_labels = request
            .records
            .iter()
            .map(|record| record.title.clone())
            .collect();
        if let Some(record_loader) = record_loader {
            for query in self.loader_queries.clone() {
                let records = record_loader(query).map_err(|error| {
                    crate::sessions::SessionsCommandError::Picker(std::io::Error::other(error))
                })?;
                self.loaded_session_ids.push(
                    records
                        .into_iter()
                        .map(|record| record.session_id)
                        .collect(),
                );
            }
        }
        Ok(Some(self.selected_outcome.clone()))
    }
}

pub(super) fn create_codex_state_db_with_threads(state_database_path: &Path, prompt_canary: &str) {
    let codex_home = match state_database_path.parent() {
        Some(codex_home) => codex_home,
        None => panic!("state database should have codex home parent"),
    };
    let test_root = match codex_home.parent() {
        Some(test_root) => test_root,
        None => panic!("codex home should have test root parent"),
    };
    create_codex_state_db_with_thread_rows(
        state_database_path,
        prompt_canary,
        &[
            CodexStateThreadFixture::new(
                "thread-older",
                &test_root.join("project-b"),
                "openai",
                "cli",
                "cli",
                "feature",
                1000,
            ),
            CodexStateThreadFixture::new(
                "thread-newer",
                &test_root.join("project-a"),
                "codex-router",
                "cli",
                "cli",
                "main",
                2000,
            ),
        ],
    );
}

pub(super) fn create_codex_state_db_with_thread_rows(
    state_database_path: &Path,
    prompt_canary: &str,
    rows: &[CodexStateThreadFixture],
) {
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    runtime.block_on(async {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(state_database_path)
            .create_if_missing(true);
        let pool = must_ok(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await,
        );
        must_ok(
            sqlx::query(
                r#"
            CREATE TABLE threads (
                id TEXT PRIMARY KEY NOT NULL,
                rollout_path TEXT,
                created_at TEXT,
                updated_at TEXT,
                source TEXT,
                model_provider TEXT,
                cwd TEXT,
                name TEXT,
                title TEXT,
                sandbox_policy TEXT,
                approval_mode TEXT,
                tokens_used INTEGER,
                has_user_event INTEGER,
                archived INTEGER NOT NULL DEFAULT 0,
                archived_at TEXT,
                git_sha TEXT,
                git_branch TEXT,
                git_origin_url TEXT,
                cli_version TEXT,
                first_user_message TEXT,
                agent_nickname TEXT,
                agent_role TEXT,
                memory_mode TEXT,
                model TEXT,
                reasoning_effort TEXT,
                agent_path TEXT,
                created_at_ms INTEGER,
                updated_at_ms INTEGER,
                thread_source TEXT,
                preview TEXT,
                recency_at TEXT,
                recency_at_ms INTEGER
            )
            "#,
            )
            .execute(&pool)
            .await,
        );
        must_ok(
            sqlx::query(
                "CREATE INDEX idx_threads_created_at_ms \
                 ON threads(created_at_ms DESC, id DESC)",
            )
            .execute(&pool)
            .await,
        );
        must_ok(
            sqlx::query(
                "CREATE INDEX idx_threads_recency_at_ms \
                 ON threads(recency_at_ms DESC, id DESC)",
            )
            .execute(&pool)
            .await,
        );
        for row in rows {
            must_ok(
                sqlx::query(
                    r#"
                INSERT INTO threads (
                    id, rollout_path, created_at, updated_at, source, model_provider, cwd,
                    name, title, sandbox_policy, approval_mode, tokens_used, has_user_event,
                    archived, archived_at, git_sha, git_branch, git_origin_url, cli_version,
                    first_user_message, agent_nickname, agent_role, memory_mode, model,
                    reasoning_effort, agent_path, created_at_ms, updated_at_ms,
                    thread_source, preview, recency_at, recency_at_ms
                ) VALUES (
                    ?, NULL, NULL, NULL, ?, ?, ?, ?, ?, NULL, NULL, 0, 1,
                    0, NULL, NULL, ?, ?, NULL,
                    ?, NULL, NULL, NULL, ?, NULL, NULL,
                    ?, ?, ?, ?, NULL, ?
                )
                "#,
                )
                .bind(&row.id)
                .bind(&row.source)
                .bind(&row.provider)
                .bind(row.cwd.display().to_string())
                .bind(&row.name)
                .bind(row.title.as_deref().unwrap_or(prompt_canary))
                .bind(&row.git_branch)
                .bind(&row.git_origin_url)
                .bind(row.first_user_message.as_deref().unwrap_or(prompt_canary))
                .bind(&row.model)
                .bind(row.recency_at_ms - 100)
                .bind(row.recency_at_ms)
                .bind(&row.thread_source)
                .bind(row.preview.as_deref().unwrap_or(prompt_canary))
                .bind(row.recency_at_ms)
                .execute(&pool)
                .await,
            );
        }
        pool.close().await;
    });
}
