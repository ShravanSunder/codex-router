//! Sealed catalog and launch stand-ins around the real compiled picker dispatcher.
use super::{
    CliContext, SessionActionSelection, SessionsCommand, SessionsCommandError,
    SessionsCommandRunner, SessionsPicker, SessionsPickerOutcome, SessionsPickerRecordLoader,
    SessionsPickerRequest, TerminalSessionsPicker, run_sessions_command_with_dependencies,
};
use std::{ffi::OsString, io, io::Write, path::PathBuf};

pub(crate) fn run_stored_model_terminal_fixture() -> io::Result<()> {
    let catalog = SealedPickerCatalog::create()?;
    let context = CliContext::new(vec![(
        "HOME".to_owned(),
        catalog.root.to_string_lossy().into_owned(),
    )])
    .with_current_dir(catalog.root.clone());
    let command = SessionsCommand::parse(vec![
        OsString::from("--any"),
        OsString::from("--local"),
        OsString::from("--provider"),
        OsString::from("any"),
    ])
    .map_err(io::Error::other)?;
    let mut picker = SealedCatalogTerminalPicker {
        terminal: TerminalSessionsPicker::for_context(&context),
    };
    let mut runner = NoLaunchMarkerRunner;
    match run_sessions_command_with_dependencies(
        &mut io::stdout(),
        command,
        &context,
        &mut runner,
        &mut picker,
    ) {
        Ok(()) => Ok(()),
        Err(SessionsCommandError::PickerCanceled) => {
            writeln!(io::stdout(), "SESSION_PICKER_OUTCOME canceled")
        }
        Err(error) => Err(io::Error::other(error)),
    }
}

struct SealedPickerCatalog {
    root: PathBuf,
}

impl SealedPickerCatalog {
    fn create() -> io::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "agent-sessions-terminal-catalog-{}",
            std::process::id()
        ));
        std::fs::create_dir(&root)?;
        let catalog = Self { root };
        let home = catalog.root.join(".codex");
        std::fs::create_dir(&home)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(home.join("state_5.sqlite"))
                .create_if_missing(true);
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .map_err(io::Error::other)?;
            let result = sqlx::raw_sql(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT,
                    model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT,
                    git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT,
                    first_user_message TEXT, created_at_ms INTEGER, updated_at_ms INTEGER,
                    archived INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);
                 CREATE INDEX idx_threads_created_at_ms ON threads(created_at_ms DESC, id DESC);",
            )
            .execute(&pool)
            .await
            .map_err(io::Error::other);
            pool.close().await;
            result.map(|_| ())
        })?;
        Ok(catalog)
    }
}

impl Drop for SealedPickerCatalog {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct SealedCatalogTerminalPicker {
    terminal: TerminalSessionsPicker,
}

impl SessionsPicker for SealedCatalogTerminalPicker {
    fn ensure_available(&self) -> Result<(), SessionsCommandError> {
        self.terminal.ensure_available()
    }

    fn select_session(
        &mut self,
        _request: SessionsPickerRequest,
        _record_loader: Option<SessionsPickerRecordLoader>,
    ) -> Result<Option<SessionsPickerOutcome>, SessionsCommandError> {
        let mut request = crate::session_picker::terminal_fixture_request();
        request.root = crate::session_picker::SessionsPickerRoot::Any;
        request.source = super::SessionsSource::All;
        let record = request
            .records
            .iter_mut()
            .find(|record| record.session_id == "thread-b")
            .ok_or_else(|| {
                SessionsCommandError::Picker(io::Error::other("sealed beta row absent"))
            })?;
        record.title = "Pointer focus beta".to_owned();
        record.preview = Some("BETA_PREVIEW_ACTIVE".to_owned());
        record.conversation.snippets = vec!["BETA_CONVERSATION_ACTIVE".to_owned()];
        record.model = Some("MALFORMED_MODEL\u{1}VALUE".to_owned());
        record.reasoning_effort = Some("high".to_owned());
        self.terminal.select_session(request, None)
    }
}

struct NoLaunchMarkerRunner;

impl SessionsCommandRunner for NoLaunchMarkerRunner {
    fn run_codex_new(&mut self, _arguments: &[OsString]) -> Result<(), SessionsCommandError> {
        writeln!(io::stdout(), "SESSION_PICKER_OUTCOME start-new")
            .map_err(SessionsCommandError::Stdout)
    }

    fn run_codex_resume(
        &mut self,
        arguments: &[OsString],
        selection: &SessionActionSelection,
    ) -> Result<(), SessionsCommandError> {
        if selection.session_id() == "thread-b" {
            let launch = codex_native_integration::SessionLaunch::resume_local(
                std::path::Path::new("/sealed-catalog"),
                arguments,
                &selection.session_id(),
                &selection.model_choice,
            );
            let projected = launch.arguments();
            if !projected.contains(&OsString::from("model_reasoning_effort=\"high\""))
                || projected.iter().any(|argument| {
                    argument
                        .to_str()
                        .is_some_and(|value| value.starts_with("model="))
                })
            {
                return Err(SessionsCommandError::Picker(io::Error::other(
                    "sealed choice must retain valid effort and discard malformed model",
                )));
            }
        }
        writeln!(
            io::stdout(),
            "SESSION_PICKER_OUTCOME resume:{}",
            selection.session_id()
        )
        .map_err(SessionsCommandError::Stdout)
    }

    fn run_codex_fork(
        &mut self,
        _arguments: &[OsString],
        selection: &SessionActionSelection,
    ) -> Result<(), SessionsCommandError> {
        writeln!(
            io::stdout(),
            "SESSION_PICKER_OUTCOME fork:{}",
            selection.session_id()
        )
        .map_err(SessionsCommandError::Stdout)
    }
}
