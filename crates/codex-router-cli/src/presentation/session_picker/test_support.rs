use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::presentation::session_picker::request::SessionsPickerRoot;
use crate::sessions::RepositoryIdentity;
use crate::sessions::SessionConversationPreview;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;

pub(crate) fn picker_request() -> SessionsPickerRequest {
    SessionsPickerRequest {
        root: SessionsPickerRoot::Cwd,
        provider: SessionsProvider::Any,
        source: SessionsSource::Interactive,
        sort: SessionsSort::Updated,
        current_dir: "/repo/project-a".into(),
        repository_identity: RepositoryIdentity {
            normalized_origin: Some("github.com/shravan-agent/codex-router".to_owned()),
            live_roots: vec!["/repo/project-a".into(), "/repo/project-b".into()],
            repository_basename: "project-a".to_owned(),
            fallback_cwd: None,
        },
        current_provider: Some("codex-router".to_owned()),
        new_session_args_display: String::new(),
        records: vec![
            picker_record(
                "thread-a",
                "Feature design session",
                "/repo/project-a",
                "codex-router",
                "cli",
            ),
            picker_record(
                "thread-b",
                "Provider migration with very very long provider metadata",
                "/repo/project-b",
                "openai-super-long-provider-id-for-width-proof",
                "cli",
            ),
            picker_record(
                "thread-sub",
                "Subagent planning",
                "/repo/project-a",
                "codex-router",
                "subagent",
            ),
        ],
    }
}

pub(crate) fn picker_record(
    session_id: &str,
    title: &str,
    cwd: &str,
    provider: &str,
    source: &str,
) -> SessionPickerRecord {
    SessionPickerRecord {
        session_id: session_id.to_owned(),
        title: title.to_owned(),
        full_title: title.to_owned(),
        recency: "now".to_owned(),
        created: "1d ago".to_owned(),
        recency_at_ms: Some(2_000),
        created_at_ms: Some(1_000),
        branch: "main".to_owned(),
        persisted_branch: "main".to_owned(),
        context: cwd.rsplit('/').next().unwrap_or(cwd).to_owned(),
        cwd: Some(cwd.to_owned()),
        normalized_cwd: Some(cwd.to_owned()),
        git_origin_url: Some("https://github.com/shravan-agent/codex-router.git".to_owned()),
        provider: Some(provider.to_owned()),
        model: Some("gpt-5-codex".to_owned()),
        preview: Some(format!("{title} preview text")),
        first_user_message: format!("{title} first real message"),
        conversation: SessionConversationPreview {
            snippets: vec![
                format!("{title} first real message"),
                format!("{title} latest assistant reply"),
            ],
            unavailable_reason: None,
        },
        conversation_source: None,
        source: Some(source.to_owned()),
        thread_source: Some(source.to_owned()),
    }
}
