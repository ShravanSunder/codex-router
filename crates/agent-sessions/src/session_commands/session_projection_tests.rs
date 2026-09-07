//! Shared fixtures for Sessions projection contracts.
use super::*;
use serde_json::json;
use sqlx::Execute;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

fn search_consistency_record(
    git_origin_url: Option<&str>,
    first_user_message: Option<&str>,
) -> SessionRecord {
    SessionRecord {
        session_id: "thread-search-consistency".to_owned(),
        rollout_path: None,
        display_title: Some("deploy rollback plan".to_owned()),
        cwd: Some("/history/app.impl-search".to_owned()),
        provider: Some("codex-router".to_owned()),
        model: None,
        source: Some("cli".to_owned()),
        thread_source: None,
        git_branch: Some("main".to_owned()),
        git_origin_url: git_origin_url.map(str::to_owned),
        name: None,
        title: None,
        preview: None,
        first_user_message: first_user_message.map(str::to_owned),
        created_at_ms: Some(1),
        updated_at_ms: Some(1),
        recency_at_ms: Some(1),
    }
}

#[path = "catalog_projection_tests.rs"]
mod catalog_projection_tests;
#[path = "conversation_preview_tests.rs"]
mod conversation_preview_tests;
#[path = "repository_scope_tests.rs"]
mod repository_scope_tests;
#[path = "session_display_tests.rs"]
mod session_display_tests;
#[path = "session_option_tests.rs"]
mod session_option_tests;
