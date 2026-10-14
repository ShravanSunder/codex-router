//! Typed input fixtures for the non-publishable compiled picker storyboard.
use super::picker_request::{SessionsPickerRequest, SessionsPickerRoot};
use super::test_support::{picker_record, picker_request};
use crate::sessions::{SessionPickerRecord, SessionsProvider, SessionsSource};
use collaboration_client::protocol::{
    ChannelDescription, ClaudeCodeInteractiveOrigin, ClaudeCodeInteractiveStatus,
    EndpointAvailability, EndpointDescription, EndpointRef, HostedProviderOrigin,
    ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderIdentity, ProviderKind, ProviderRuntimeIdentity,
    ProviderSessionState, ProviderSessionSummary, ProviderTransport, SessionRef,
};
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURE_SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const SHARED_SESSION: &str = "00000000-0000-4000-8000-000000000011";

pub(super) fn provider_filter_storyboard_request() -> std::io::Result<SessionsPickerRequest> {
    let mut request = picker_request();
    request.root = SessionsPickerRoot::Any;
    request.provider = SessionsProvider::Any;
    request.source = SessionsSource::All;
    let observed_seconds = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_secs(),
    )
    .map_err(std::io::Error::other)?;
    let mut codex_alpha = picker_record(
        SHARED_SESSION,
        "Codex Alpha",
        "/repo/project-a",
        "claude",
        "cli",
    );
    let mut codex_beta = picker_record(
        "00000000-0000-4000-8000-000000000012",
        "Codex Beta",
        "/repo/project-a",
        "codex-router",
        "cli",
    );
    for record in [&mut codex_alpha, &mut codex_beta] {
        record.recency_at_ms = Some(observed_seconds.saturating_mul(1_000));
    }
    let claude_endpoint = fixture_endpoint("claude-fixture")?;
    let claude_gamma = managed_summary(claude_endpoint.clone(), SHARED_SESSION, observed_seconds)?;
    let claude_gamma = SessionPickerRecord::from_provider_summary(&claude_gamma, "Claude Gamma")
        .with_provider_endpoint(
            &claude_gamma,
            &provider_endpoint(claude_endpoint.clone(), ProviderKind::ClaudeCode)?,
        );
    let claude_delta = ProviderSessionSummary::ClaudeCodeInteractive {
        origin: ClaudeCodeInteractiveOrigin::ClaudeCodeInteractive,
        target: SessionRef {
            endpoint: claude_endpoint.clone(),
            session_id: "claude-delta"
                .to_owned()
                .try_into()
                .map_err(std::io::Error::other)?,
        },
        name: Some("Claude Delta".to_owned()),
        working_directory: "/repo/project-a"
            .to_owned()
            .try_into()
            .map_err(std::io::Error::other)?,
        status: ClaudeCodeInteractiveStatus::Idle,
        started_at: observed_seconds,
        updated_at: observed_seconds,
        status_updated_at: Some(observed_seconds),
        kind: "interactive".to_owned(),
        entrypoint: "storyboard-fixture".to_owned(),
    };
    let claude_delta = SessionPickerRecord::from_provider_summary(&claude_delta, "Claude Delta")
        .with_provider_endpoint(
            &claude_delta,
            &provider_endpoint(claude_endpoint, ProviderKind::ClaudeCode)?,
        );
    let cursor_endpoint = fixture_endpoint("cursor-fixture")?;
    let cursor_echo = managed_summary(cursor_endpoint.clone(), SHARED_SESSION, observed_seconds)?;
    let cursor_echo = SessionPickerRecord::from_provider_summary(&cursor_echo, "Cursor Echo")
        .with_provider_endpoint(
            &cursor_echo,
            &provider_endpoint(cursor_endpoint, ProviderKind::Cursor)?,
        );
    let unknown = managed_summary(
        fixture_endpoint("unknown-fixture")?,
        "unknown-zeta",
        observed_seconds,
    )?;
    // The label deliberately resembles Claude. No runtime-kind evidence exists.
    let unknown = SessionPickerRecord::from_provider_summary(&unknown, "Claude-lookalike Zeta");
    request.records = vec![
        codex_alpha,
        codex_beta,
        claude_gamma,
        claude_delta,
        cursor_echo,
        unknown,
    ];
    Ok(request)
}

fn fixture_endpoint(endpoint_id: &str) -> std::io::Result<EndpointRef> {
    Ok(EndpointRef {
        service_id: FIXTURE_SERVICE
            .to_owned()
            .try_into()
            .map_err(std::io::Error::other)?,
        endpoint_id: endpoint_id
            .to_owned()
            .try_into()
            .map_err(std::io::Error::other)?,
    })
}

fn managed_summary(
    endpoint: EndpointRef,
    session_id: &str,
    observed_seconds: i64,
) -> std::io::Result<ProviderSessionSummary> {
    let creator = ProviderIdentity::Session(SessionRef {
        endpoint: fixture_endpoint("codex-fixture")?,
        session_id: "fixture-creator"
            .to_owned()
            .try_into()
            .map_err(std::io::Error::other)?,
    });
    Ok(ProviderSessionSummary::HostedProvider {
        origin: Some(HostedProviderOrigin::HostedProvider),
        target: SessionRef {
            endpoint,
            session_id: session_id
                .to_owned()
                .try_into()
                .map_err(std::io::Error::other)?,
        },
        working_directory: "/repo/project-a"
            .to_owned()
            .try_into()
            .map_err(std::io::Error::other)?,
        updated_at: observed_seconds,
        state: ProviderSessionState::Idle,
        approver: creator.clone(),
        created_by: creator,
    })
}

fn provider_endpoint(
    endpoint: EndpointRef,
    provider: ProviderKind,
) -> std::io::Result<EndpointDescription> {
    let capabilities = ProviderCapabilities::try_from(vec![ProviderCapability {
        name: ProviderCapabilityName::Prompt,
        status: ProviderCapabilityStatus::Unsupported,
        evidence: ProviderCapabilityEvidence::NotAvailable,
    }])
    .map_err(std::io::Error::other)?;
    Ok(EndpointDescription {
        endpoint,
        label: "Typed provider fixture"
            .to_owned()
            .try_into()
            .map_err(std::io::Error::other)?,
        availability: EndpointAvailability::Unprobed,
        channels: vec![ChannelDescription::ExternalProvider {
            transport: ProviderTransport::StdioAcp,
            binding_id: "fixture-binding"
                .to_owned()
                .try_into()
                .map_err(std::io::Error::other)?,
            binding_generation: 1_u64.try_into().map_err(std::io::Error::other)?,
            runtime: ProviderRuntimeIdentity {
                provider,
                runtime_name: "storyboard-input"
                    .to_owned()
                    .try_into()
                    .map_err(std::io::Error::other)?,
                runtime_version: None,
            },
            capabilities,
        }],
    })
}
