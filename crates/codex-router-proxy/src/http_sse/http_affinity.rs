use super::*;

pub(super) struct AffinityOwnerRecordingBody {
    inner: Box<dyn Read + Send>,
    recorder: Arc<dyn HttpAffinityOwnerRecorder>,
    affinity_secret: RouterAffinityHashSecret,
    account_id: AccountId,
    credential_generation: u64,
    buffered: Vec<u8>,
    recorded: bool,
}

impl AffinityOwnerRecordingBody {
    pub(super) fn new(
        inner: Box<dyn Read + Send>,
        recorder: Arc<dyn HttpAffinityOwnerRecorder>,
        affinity_secret: RouterAffinityHashSecret,
        account_id: AccountId,
        credential_generation: u64,
    ) -> Self {
        Self {
            inner,
            recorder,
            affinity_secret,
            account_id,
            credential_generation,
            buffered: Vec::new(),
            recorded: false,
        }
    }

    fn record_if_ready(&mut self) -> io::Result<()> {
        if self.recorded {
            return Ok(());
        }
        let Some(response_id) = extract_response_id_from_body(&self.buffered)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
        else {
            return Ok(());
        };
        self.recorded = true;
        let affinity_key_hash = hash_previous_response_id(&self.affinity_secret, &response_id)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let owner = PreviousResponseAffinityOwnerRecord::new(
            affinity_key_hash,
            self.account_id.clone(),
            self.credential_generation,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            current_unix_seconds(),
        );
        self.recorder
            .record_affinity_owner(&owner)
            .map_err(|error| io::Error::other(error.to_string()))
    }
}

impl Read for AffinityOwnerRecordingBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if read == 0 {
            self.record_if_ready()?;
            return Ok(0);
        }
        let Some(read_buffer) = buffer.get(..read) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "read length exceeded buffer length",
            ));
        };
        self.buffered.extend_from_slice(read_buffer);
        self.record_if_ready()?;
        Ok(read)
    }
}

pub(crate) fn redacted_account_hash(account_id: &AccountId) -> String {
    let mut hasher = DefaultHasher::new();
    account_id.as_str().hash(&mut hasher);

    format!("acct_{:016x}", hasher.finish())
}

pub(crate) const fn local_auth_audit_result(reason: LocalAuthError) -> LocalAuthAuditResult {
    match reason {
        LocalAuthError::Missing => LocalAuthAuditResult::Missing,
        LocalAuthError::Empty => LocalAuthAuditResult::Empty,
        LocalAuthError::Old => LocalAuthAuditResult::Old,
        LocalAuthError::Wrong => LocalAuthAuditResult::Wrong,
    }
}

pub(crate) const fn local_auth_decision_reason(reason: LocalAuthError) -> &'static str {
    match reason {
        LocalAuthError::Missing => "local_auth_missing",
        LocalAuthError::Empty => "local_auth_empty",
        LocalAuthError::Old => "local_auth_old",
        LocalAuthError::Wrong => "local_auth_wrong",
    }
}

pub(crate) fn local_auth_rejection_audit_event(
    transport_kind: TransportKind,
    route_kind: AuditRouteKind,
    reason: LocalAuthError,
) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind,
        local_auth_result: local_auth_audit_result(reason),
        outcome: AuditOutcome::Rejected,
        decision_reason: local_auth_decision_reason(reason),
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: None,
        error_class: Some("local_auth"),
    })
}

pub(crate) fn allowed_audit_event(
    transport_kind: TransportKind,
    route_kind: AuditRouteKind,
    account_hash: String,
) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Allowed,
        decision_reason: "forwarded",
        response_commit_state: ResponseCommitState::Committed,
        account_hash: Some(account_hash),
        error_class: None,
    })
}

pub(super) fn http_selection_rejection_audit_event(route_kind: AuditRouteKind) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind: TransportKind::Http,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "selection_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: None,
        error_class: Some("selection"),
    })
}

pub(super) fn http_credential_rejection_audit_event(
    route_kind: AuditRouteKind,
    account_hash: String,
) -> AuditEvent {
    AuditEvent::proxy_decision(AuditEventFields {
        request_id: RequestId::new("local_proxy_request"),
        route_kind,
        transport_kind: TransportKind::Http,
        local_auth_result: LocalAuthAuditResult::Valid,
        outcome: AuditOutcome::Rejected,
        decision_reason: "credential_rejected",
        response_commit_state: ResponseCommitState::NotCommitted,
        account_hash: Some(account_hash),
        error_class: Some("provider_credential"),
    })
}

pub(super) fn audit_route_kind_for_request(request: &HttpProxyRequest) -> AuditRouteKind {
    match request_route_kind(request) {
        Ok(route_kind) => audit_route_kind_for_route_kind(route_kind),
        Err(_error) => AuditRouteKind::Responses,
    }
}

pub(super) fn audit_route_kind_for_route_kind(route_kind: RouteKind) -> AuditRouteKind {
    match route_kind {
        RouteKind::Responses => AuditRouteKind::Responses,
        RouteKind::ClaudeMessages => AuditRouteKind::ClaudeMessages,
        RouteKind::ResponsesWebSocket => AuditRouteKind::ResponsesWebSocket,
        RouteKind::Models => AuditRouteKind::Models,
        RouteKind::MemoriesTraceSummarize => AuditRouteKind::MemoryTrace,
        RouteKind::ResponsesCompact => AuditRouteKind::Compact,
        RouteKind::ImageGenerations => AuditRouteKind::ImageGenerations,
        RouteKind::ImageEdits => AuditRouteKind::ImageEdits,
    }
}

pub(crate) fn extract_response_id_from_body(
    body: &[u8],
) -> Result<Option<PreviousResponseId>, HttpProxyError> {
    if let Some(response_id) = extract_json_response_id(body)? {
        return Ok(Some(response_id));
    }

    for line in body.split(|byte| *byte == b'\n') {
        let line = trim_ascii(line);
        let Some(data) = line.strip_prefix(b"data:") else {
            continue;
        };
        let data = trim_ascii(data);
        if data == b"[DONE]" || data.is_empty() {
            continue;
        }
        if let Some(response_id) = extract_json_response_id(data)? {
            return Ok(Some(response_id));
        }
    }

    Ok(None)
}

pub(super) fn extract_json_response_id(
    body: &[u8],
) -> Result<Option<PreviousResponseId>, HttpProxyError> {
    let value = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) => value,
        Err(_error) => return Ok(None),
    };
    let Some(response_id) = value.get("id").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    if response_id.is_empty() {
        return Ok(None);
    }
    PreviousResponseId::new(response_id.to_owned())
        .map(Some)
        .map_err(|_error| HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
        })
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    match bytes.get(start..end) {
        Some(trimmed) => trimmed,
        None => &[],
    }
}

pub(super) fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(super) fn request_route_kind(request: &HttpProxyRequest) -> Result<RouteKind, HttpProxyError> {
    match classify_route(
        request.method(),
        path_without_query(request.path()),
        request.websocket_upgrade(),
    ) {
        RouteClass::Supported(route_kind) => Ok(route_kind),
        RouteClass::Rejected { reason } => Err(HttpProxyError::Rejected { reason }),
    }
}

/// Resolves the provider profile needed by HTTP credential resolution.
/// Keep this mapping aligned with the selector's private route-profile mapping.
pub(super) fn http_route_profile_for_kind(route_kind: RouteKind) -> RouteProfile {
    match route_kind {
        RouteKind::ResponsesWebSocket => RESPONSES_WEBSOCKET.clone(),
        RouteKind::ClaudeMessages => CLAUDE_MESSAGES.clone(),
        RouteKind::Responses
        | RouteKind::Models
        | RouteKind::MemoriesTraceSummarize
        | RouteKind::ResponsesCompact
        | RouteKind::ImageGenerations
        | RouteKind::ImageEdits => RESPONSES_HTTP.clone(),
    }
}

pub(super) fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map_or(path, |(path_component, _query)| path_component)
}
