use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSource;

pub(crate) fn picker_request() -> SessionsPickerRequest {
    SessionsPickerRequest {
        root: SessionsRoot::Cwd,
        provider: SessionsProvider::Any,
        source: SessionsSource::Interactive,
        current_dir: "/repo/project-a".into(),
        current_provider: Some("codex-router".to_owned()),
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

fn picker_record(
    session_id: &str,
    title: &str,
    cwd: &str,
    provider: &str,
    source: &str,
) -> SessionPickerRecord {
    SessionPickerRecord {
        session_id: session_id.to_owned(),
        title: title.to_owned(),
        recency: "now".to_owned(),
        branch: "main".to_owned(),
        context: cwd.rsplit('/').next().unwrap_or(cwd).to_owned(),
        cwd: Some(cwd.to_owned()),
        provider: Some(provider.to_owned()),
        model: Some("gpt-5-codex".to_owned()),
        preview: Some(format!("{title} preview text")),
        source: Some(source.to_owned()),
        thread_source: Some(source.to_owned()),
    }
}
