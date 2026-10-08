//! Routed Claude Code process launch and transcript inventory.

use super::session_launch_selection::collaboration_service_directory;
use super::{CliContext, SessionsCommandError};
use collaboration_client::protocol::{
    EndpointRef, NativeSessionScope, NativeSessionSource, NativeSessionView,
    ProviderSessionListParams, ProviderSessionSummary, ServiceManifest,
};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::Read as _,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const ROUTER_PROXY_ENDPOINT_NOT_PUBLISHED: &str =
    "Router proxy endpoint not published; restart the Router Host";
const ANTHROPIC_API_PATH: &str = "/anthropic";
const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
const LOCAL_ROUTER_TOKEN_FILE: &str = "local_router_token.secret";
const CLAUDE_PROJECTS_DIRECTORY: &str = ".claude/projects";
const ROUTER_HEALTH_PATH: &str = "/healthz";
const ROUTER_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HTTP_STATUS_LINE_BYTES: usize = 256;
const MAX_SERVICE_MANIFEST_BYTES: u64 = 65_536;
const PROVIDER_SESSION_PAGE_SIZE: u32 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClaudeLaunchTarget {
    service_directory: PathBuf,
    secret_root: PathBuf,
    claude_projects_directory: Option<PathBuf>,
}

pub(super) struct RoutedClaudeEnvironment {
    base_url: String,
    auth_token: String,
}

impl ClaudeLaunchTarget {
    pub(super) fn resolve(context: &CliContext) -> Result<Self, SessionsCommandError> {
        let service_directory = collaboration_service_directory(context)?;
        let router_root = service_directory
            .parent()
            .ok_or_else(|| {
                SessionsCommandError::ClaudeLaunch(
                    "Router service directory has no parent root".to_owned(),
                )
            })?
            .to_path_buf();
        let claude_projects_directory = context
            .env_var("HOME")
            .map(PathBuf::from)
            .filter(|home_directory| home_directory.is_absolute())
            .map(|home_directory| home_directory.join(CLAUDE_PROJECTS_DIRECTORY));

        Ok(Self {
            service_directory,
            secret_root: router_root.join("secrets"),
            claude_projects_directory,
        })
    }

    pub(super) fn launch_arguments(
        &self,
        session_id: Option<&str>,
        passthrough_arguments: &[OsString],
    ) -> Vec<OsString> {
        let mut arguments = Vec::with_capacity(
            passthrough_arguments
                .len()
                .saturating_add(if session_id.is_some() { 2 } else { 0 }),
        );
        if let Some(session_id) = session_id {
            arguments.push(OsString::from("--resume"));
            arguments.push(OsString::from(session_id));
        }
        arguments.extend(passthrough_arguments.iter().cloned());
        arguments
    }

    pub(super) async fn routed_environment(
        &self,
    ) -> Result<RoutedClaudeEnvironment, SessionsCommandError> {
        let router_endpoint = self.router_proxy_endpoint().await?;
        preflight_router_health(router_endpoint)
            .await
            .map_err(SessionsCommandError::ClaudeLaunch)?;
        let auth_token = self.local_router_token().await?;

        Ok(RoutedClaudeEnvironment {
            base_url: format!("http://{router_endpoint}{ANTHROPIC_API_PATH}"),
            auth_token,
        })
    }

    pub(super) async fn session_records(
        &self,
        limit: usize,
    ) -> Result<(Vec<serde_json::Value>, Option<String>), SessionsCommandError> {
        let stored_records = self.stored_transcript_records().await?;
        let (active_records, active_error) = match self.active_session_records(limit).await {
            Ok(records) => (records, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let mut records = active_records;
        records.extend(stored_records);
        records.sort_by_key(|record| std::cmp::Reverse(record_updated_at_seconds(record)));
        records.truncate(limit);
        Ok((records, active_error))
    }

    pub(super) async fn resume_working_directory(&self, session_id: &str) -> Option<PathBuf> {
        let Ok((records, _active_error)) = self.session_records(usize::MAX).await else {
            return None;
        };
        recorded_working_directory(&records, session_id)
    }

    pub(super) fn command(
        &self,
        session_id: Option<&str>,
        passthrough_arguments: &[OsString],
        working_directory: Option<&Path>,
        environment: RoutedClaudeEnvironment,
    ) -> Command {
        let mut command = Command::new("claude");
        command
            .args(self.launch_arguments(session_id, passthrough_arguments))
            .env(ANTHROPIC_BASE_URL_ENV, environment.base_url)
            .env(ANTHROPIC_AUTH_TOKEN_ENV, environment.auth_token);
        if let Some(working_directory) = working_directory {
            command.current_dir(working_directory);
        }
        command
    }

    async fn router_proxy_endpoint(&self) -> Result<SocketAddr, SessionsCommandError> {
        let manifest_path = self.service_directory.join("service.json");
        tokio::task::spawn_blocking(move || parse_router_proxy_endpoint(&manifest_path))
            .await
            .map_err(|error| {
                SessionsCommandError::ClaudeLaunch(format!(
                    "failed to read Router service discovery: {error}"
                ))
            })?
            .map_err(SessionsCommandError::ClaudeLaunch)
    }

    async fn local_router_token(&self) -> Result<String, SessionsCommandError> {
        let token_path = self.secret_root.join(LOCAL_ROUTER_TOKEN_FILE);
        let token = tokio::task::spawn_blocking(move || {
            let metadata = fs::symlink_metadata(&token_path)
                .map_err(|_| "Router is not ready: the local Router token is missing".to_owned())?;
            if !metadata.file_type().is_file() {
                return Err("Router is not ready: the local Router token is unavailable".to_owned());
            }
            let token = fs::read_to_string(&token_path).map_err(|_| {
                "Router is not ready: the local Router token is unavailable".to_owned()
            })?;
            if token.is_empty() {
                return Err("Router is not ready: the local Router token is empty".to_owned());
            }
            Ok::<_, String>(token)
        })
        .await
        .map_err(|error| {
            SessionsCommandError::ClaudeLaunch(format!(
                "Router is not ready: failed to read the local Router token: {error}"
            ))
        })?
        .map_err(SessionsCommandError::ClaudeLaunch)?;
        Ok(token)
    }

    async fn stored_transcript_records(
        &self,
    ) -> Result<Vec<serde_json::Value>, SessionsCommandError> {
        let Some(projects_directory) = self.claude_projects_directory.clone() else {
            return Ok(Vec::new());
        };
        tokio::task::spawn_blocking(move || list_stored_transcript_records(&projects_directory))
            .await
            .map_err(|error| {
                SessionsCommandError::ClaudeLaunch(format!(
                    "failed to list stored Claude transcripts: {error}"
                ))
            })?
            .map_err(SessionsCommandError::ClaudeLaunch)
    }

    async fn active_session_records(&self, limit: usize) -> Result<Vec<serde_json::Value>, String> {
        let client = collaboration_client::CollaborationClient::connect(
            &self.service_directory,
            "agent-sessions-claude-list",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|error| error.to_string())?;
        let endpoint_id = String::from("claude-local")
            .try_into()
            .map_err(|_| "Claude endpoint ID is invalid".to_owned())?;
        let endpoint = EndpointRef {
            service_id: client.identity().service_id.clone(),
            endpoint_id,
        };
        let mut records = Vec::new();
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        while records.len() < limit {
            let page = client
                .list_provider_sessions(ProviderSessionListParams {
                    endpoint: endpoint.clone(),
                    view: NativeSessionView::Active,
                    scope: NativeSessionScope::Any,
                    source: NativeSessionSource::Interactive,
                    query: None,
                    page_size: PROVIDER_SESSION_PAGE_SIZE,
                    cursor: cursor.clone(),
                })
                .await
                .map_err(|error| error.to_string())?;
            for session in page.sessions {
                if let Some(record) = active_claude_session_record(session) {
                    records.push(record);
                    if records.len() >= limit {
                        break;
                    }
                }
            }
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err("Claude session inventory returned a repeated cursor".to_owned());
            }
            cursor = Some(next_cursor);
        }
        Ok(records)
    }
}

fn active_claude_session_record(session: ProviderSessionSummary) -> Option<serde_json::Value> {
    let ProviderSessionSummary::ClaudeCodeInteractive {
        target,
        name,
        working_directory,
        status,
        started_at,
        updated_at,
        status_updated_at,
        kind,
        entrypoint,
        ..
    } = session
    else {
        return None;
    };
    Some(serde_json::json!({
        "source": "active",
        "sessionId": String::from(target.session_id),
        "name": name,
        "workingDirectory": String::from(working_directory),
        "status": status,
        "startedAtUnixSeconds": started_at,
        "updatedAtUnixSeconds": updated_at,
        "statusUpdatedAtUnixSeconds": status_updated_at,
        "kind": kind,
        "entrypoint": entrypoint,
    }))
}

pub(super) fn parse_router_proxy_endpoint(manifest_path: &Path) -> Result<SocketAddr, String> {
    let manifest_metadata = fs::symlink_metadata(manifest_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ROUTER_PROXY_ENDPOINT_NOT_PUBLISHED.to_owned()
        } else {
            format!("failed to read Router service discovery: {error}")
        }
    })?;
    if !manifest_metadata.is_file() || manifest_metadata.len() > MAX_SERVICE_MANIFEST_BYTES {
        return Err("Router service discovery is invalid; restart the Router Host".to_owned());
    }

    let mut manifest_bytes = Vec::new();
    fs::File::open(manifest_path)
        .and_then(|file| {
            file.take(MAX_SERVICE_MANIFEST_BYTES + 1)
                .read_to_end(&mut manifest_bytes)
        })
        .map_err(|error| format!("failed to read Router service discovery: {error}"))?;
    if manifest_bytes.len() as u64 > MAX_SERVICE_MANIFEST_BYTES {
        return Err("Router service discovery is invalid; restart the Router Host".to_owned());
    }
    let manifest: ServiceManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| "Router service discovery is invalid; restart the Router Host".to_owned())?;
    manifest
        .router_proxy_endpoint
        .ok_or_else(|| ROUTER_PROXY_ENDPOINT_NOT_PUBLISHED.to_owned())
}

pub(super) async fn preflight_router_health(endpoint: SocketAddr) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + ROUTER_PREFLIGHT_TIMEOUT;
    let mut stream = tokio::time::timeout_at(deadline, TcpStream::connect(endpoint))
        .await
        .map_err(|_| "Router is not ready: connection timed out".to_owned())?
        .map_err(|error| format!("Router is not ready: {error}"))?;
    let request = format!(
        "GET {ROUTER_HEALTH_PATH} HTTP/1.1\r\nhost: {endpoint}\r\nconnection: close\r\n\r\n"
    );
    tokio::time::timeout_at(deadline, stream.write_all(request.as_bytes()))
        .await
        .map_err(|_| "Router is not ready: /healthz request timed out".to_owned())?
        .map_err(|error| format!("Router is not ready: /healthz request failed: {error}"))?;

    let mut status_line = Vec::with_capacity(64);
    while status_line.len() < MAX_HTTP_STATUS_LINE_BYTES {
        let mut byte = [0_u8; 1];
        let bytes_read = tokio::time::timeout_at(deadline, stream.read(&mut byte))
            .await
            .map_err(|_| "Router is not ready: /healthz response timed out".to_owned())?
            .map_err(|error| format!("Router is not ready: /healthz response failed: {error}"))?;
        if bytes_read == 0 {
            return Err("Router is not ready: /healthz returned no response".to_owned());
        }
        status_line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    if !is_successful_http_status_line(&status_line) {
        return Err("Router is not ready: /healthz did not return HTTP 200".to_owned());
    }
    Ok(())
}

fn is_successful_http_status_line(status_line: &[u8]) -> bool {
    let Ok(status_line) = std::str::from_utf8(status_line) else {
        return false;
    };
    let mut fields = status_line.split_ascii_whitespace();
    fields
        .next()
        .is_some_and(|version| version.starts_with("HTTP/"))
        && fields.next() == Some("200")
}

fn list_stored_transcript_records(
    projects_directory: &Path,
) -> Result<Vec<serde_json::Value>, String> {
    let project_directories = match fs::read_dir(projects_directory) {
        Ok(project_directories) => project_directories,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Claude projects directory unavailable: {error}")),
    };
    let mut records = Vec::new();
    for project_entry in project_directories {
        let project_entry =
            project_entry.map_err(|error| format!("Claude project entry unavailable: {error}"))?;
        if !project_entry
            .file_type()
            .map_err(|error| format!("Claude project entry metadata unavailable: {error}"))?
            .is_dir()
        {
            continue;
        }
        let project_name = project_entry.file_name().to_string_lossy().into_owned();
        for transcript_entry in fs::read_dir(project_entry.path())
            .map_err(|error| format!("Claude project transcripts unavailable: {error}"))?
        {
            let transcript_entry = transcript_entry
                .map_err(|error| format!("Claude transcript entry unavailable: {error}"))?;
            if !transcript_entry
                .file_type()
                .map_err(|error| format!("Claude transcript metadata unavailable: {error}"))?
                .is_file()
            {
                continue;
            }
            let transcript_path = transcript_entry.path();
            if transcript_path
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("jsonl")
            {
                continue;
            }
            let Some(session_id) = transcript_path.file_stem().and_then(|stem| stem.to_str())
            else {
                continue;
            };
            if !is_canonical_session_uuid(session_id) {
                continue;
            }
            let modified_at_unix_seconds = transcript_entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|duration| i64::try_from(duration.as_secs()).ok());
            records.push(serde_json::json!({
                "source": "storedTranscript",
                "sessionId": session_id,
                "project": project_name,
                "updatedAtUnixSeconds": modified_at_unix_seconds,
            }));
        }
    }
    Ok(records)
}

fn recorded_working_directory(records: &[serde_json::Value], session_id: &str) -> Option<PathBuf> {
    records
        .iter()
        .filter(|record| {
            record.get("sessionId").and_then(serde_json::Value::as_str) == Some(session_id)
        })
        .find_map(|record| {
            record
                .get("workingDirectory")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .filter(|working_directory| working_directory.is_absolute())
        })
}

fn is_canonical_session_uuid(session_id: &str) -> bool {
    let bytes = session_id.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes.get(index) == Some(&b'-'))
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

fn record_updated_at_seconds(record: &serde_json::Value) -> i64 {
    record
        .get("updatedAtUnixSeconds")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "claude_launch_target_tests.rs"]
mod tests;
