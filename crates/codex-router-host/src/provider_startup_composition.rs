//! Isolated provider admission and endpoint publication for one Host start.
use crate::{
    ExternalProviderBinding, ExternalProviderLaunchBinding, ExternalProviderRuntime,
    ExternalProviderSupervisor,
};
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, EndpointAvailability, EndpointDescription, EndpointId,
    EndpointRef, GenerationNumber, NonEmptyText, ObservationTimestamp, ProviderBindingId,
    ProviderBindingIdentity, ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence,
    ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind, ProviderRuntimeIdentity,
    ProviderTransport, UuidIdentity,
};
use collaboration_service::{ProviderOperationStore, new_service_uuid};
use std::{io, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalProviderStartup {
    Launch(ExternalProviderLaunchBinding),
    Unavailable {
        provider: ProviderKind,
        reason: NonEmptyText,
        fix: NonEmptyText,
    },
}

impl ExternalProviderStartup {
    pub fn unavailable(
        provider: ProviderKind,
        reason: String,
        fix: String,
    ) -> Result<Self, &'static str> {
        Ok(Self::Unavailable {
            provider,
            reason: NonEmptyText::try_from(reason)?,
            fix: NonEmptyText::try_from(fix)?,
        })
    }
}

pub(crate) struct ProviderStartupComposition {
    pub supervisor: Option<Arc<ExternalProviderSupervisor>>,
    pub endpoints: Vec<EndpointDescription>,
    pub retirements: Vec<(CancellationToken, EndpointDescription)>,
}

pub(crate) async fn compose_provider_startup(
    configured: Vec<ExternalProviderStartup>,
    store: Option<Arc<Mutex<ProviderOperationStore>>>,
    service_id: &UuidIdentity,
    service_epoch: &UuidIdentity,
    mcp_url: &str,
) -> io::Result<ProviderStartupComposition> {
    let mut bindings = Vec::new();
    let mut endpoints = Vec::with_capacity(configured.len());
    let mut retirements = Vec::new();
    for provider in configured {
        match provider {
            ExternalProviderStartup::Unavailable {
                provider,
                reason,
                fix,
            } => {
                endpoints.push(unavailable_endpoint(service_id, provider, reason, fix)?);
            }
            ExternalProviderStartup::Launch(binding) if store.is_none() => {
                endpoints.push(unavailable_endpoint(
                    service_id,
                    binding.provider,
                    text("provider operation storage unavailable")?,
                    text("repair the router root storage and restart the Host")?,
                )?);
            }
            ExternalProviderStartup::Launch(binding) => {
                let executable = binding.launch.executable.display().to_string();
                let runtime = ExternalProviderRuntime::initialize_with_mcp_http(
                    binding.launch,
                    "router-collaboration",
                    mcp_url.to_owned(),
                )
                .await;
                match runtime {
                    Ok(runtime) => {
                        let endpoint = EndpointRef {
                            service_id: service_id.clone(),
                            endpoint_id: binding.endpoint_id,
                        };
                        let generation_number = generation_number(binding.provider)?;
                        let runtime_name =
                            runtime.admission().runtime_name.clone().unwrap_or_else(|| {
                                match binding.provider {
                                    ProviderKind::ClaudeCode => "claude-code",
                                    ProviderKind::Cursor => "cursor",
                                }
                                .to_owned()
                            });
                        let runtime_identity = ProviderRuntimeIdentity {
                            provider: binding.provider,
                            runtime_name: text(&runtime_name)?,
                            runtime_version: runtime
                                .admission()
                                .runtime_version
                                .clone()
                                .map(NonEmptyText::try_from)
                                .transpose()
                                .map_err(io::Error::other)?,
                        };
                        let capabilities = provider_capabilities(
                            runtime.admission().supports_load,
                            runtime.admission().supports_mcp_http,
                        )?;
                        let binding_id =
                            ProviderBindingId::try_from(String::from(new_service_uuid()?))
                                .map_err(io::Error::other)?;
                        let generation = CodexGeneration {
                            service_epoch: service_epoch.clone(),
                            generation: generation_number,
                        };
                        let identity = ProviderBindingIdentity {
                            endpoint: endpoint.clone(),
                            binding_id: binding_id.clone(),
                            runtime: runtime_identity.clone(),
                            transport: ProviderTransport::StdioAcp,
                            generation,
                            capabilities: capabilities.clone(),
                        };
                        let description = EndpointDescription {
                            endpoint,
                            label: binding.label,
                            availability: EndpointAvailability::Available {
                                observed_at: observed_at()?,
                            },
                            channels: vec![ChannelDescription::ExternalProvider {
                                transport: ProviderTransport::StdioAcp,
                                binding_id,
                                binding_generation: generation_number,
                                runtime: runtime_identity,
                                capabilities,
                            }],
                        };
                        retirements.push((runtime.retirement(), description.clone()));
                        endpoints.push(description);
                        bindings.push(ExternalProviderBinding { identity, runtime });
                    }
                    Err(error) => {
                        let fix = match binding.provider {
                            ProviderKind::ClaudeCode => {
                                "install claude-agent-acp or set the Claude executable in providers.json or --claude-acp-executable"
                            }
                            ProviderKind::Cursor => {
                                "install Cursor agent or set the Cursor executable in providers.json or --cursor-acp-executable"
                            }
                        };
                        endpoints.push(unavailable_endpoint(
                            service_id,
                            binding.provider,
                            text(&provider_startup_failure_reason(&executable, &error))?,
                            text(fix)?,
                        )?);
                    }
                }
            }
        }
    }
    let supervisor = match store {
        Some(store) if !endpoints.is_empty() => Some(Arc::new(
            ExternalProviderSupervisor::new(bindings, store).map_err(io::Error::other)?,
        )),
        _ => None,
    };
    Ok(ProviderStartupComposition {
        supervisor,
        endpoints,
        retirements,
    })
}

fn provider_startup_failure_reason(
    executable: &str,
    error: &crate::ExternalProviderRuntimeError,
) -> String {
    format!("provider launch or initialize failed for {executable}: {error}")
}

fn unavailable_endpoint(
    service_id: &UuidIdentity,
    provider: ProviderKind,
    reason: NonEmptyText,
    fix: NonEmptyText,
) -> io::Result<EndpointDescription> {
    let (endpoint_id, label) = match provider {
        ProviderKind::ClaudeCode => ("claude-local", "Claude Code"),
        ProviderKind::Cursor => ("cursor-local", "Cursor"),
    };
    Ok(EndpointDescription {
        endpoint: EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).map_err(io::Error::other)?,
        },
        label: text(label)?,
        availability: EndpointAvailability::Unavailable {
            observed_at: observed_at()?,
            reason,
            fix: Some(fix),
        },
        channels: Vec::new(),
    })
}

fn observed_at() -> io::Result<ObservationTimestamp> {
    ObservationTimestamp::try_from(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
    .map_err(io::Error::other)
}

fn text(value: &str) -> io::Result<NonEmptyText> {
    NonEmptyText::try_from(value.to_owned()).map_err(io::Error::other)
}

fn generation_number(provider: ProviderKind) -> io::Result<GenerationNumber> {
    let ordinal = match provider {
        ProviderKind::ClaudeCode => 1,
        ProviderKind::Cursor => 2,
    };
    GenerationNumber::try_from(ordinal).map_err(io::Error::other)
}

fn provider_capabilities(
    supports_load: bool,
    supports_mcp_http: bool,
) -> io::Result<ProviderCapabilities> {
    ProviderCapabilities::try_from(vec![
        ProviderCapability {
            name: ProviderCapabilityName::Create,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        },
        ProviderCapability {
            name: ProviderCapabilityName::Prompt,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        },
        ProviderCapability {
            name: ProviderCapabilityName::Load,
            status: if supports_load {
                ProviderCapabilityStatus::Supported
            } else {
                ProviderCapabilityStatus::Unsupported
            },
            evidence: if supports_load {
                ProviderCapabilityEvidence::Advertised
            } else {
                ProviderCapabilityEvidence::NotAvailable
            },
        },
        ProviderCapability {
            name: ProviderCapabilityName::Cancel,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        },
        ProviderCapability {
            name: ProviderCapabilityName::Permissions,
            status: ProviderCapabilityStatus::Unverified,
            evidence: ProviderCapabilityEvidence::NotAvailable,
        },
        ProviderCapability {
            name: ProviderCapabilityName::CollaborationMcp,
            status: if supports_mcp_http {
                ProviderCapabilityStatus::Supported
            } else {
                ProviderCapabilityStatus::Unsupported
            },
            evidence: if supports_mcp_http {
                ProviderCapabilityEvidence::Advertised
            } else {
                ProviderCapabilityEvidence::NotAvailable
            },
        },
        ProviderCapability {
            name: ProviderCapabilityName::CallerDetach,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::RouterQualified,
        },
    ])
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_startup_reason_preserves_sanitized_failure_metadata() {
        let account_sentinel = "synthetic-account-sentinel@example.invalid";
        let token_sentinel = "synthetic-token-sentinel-7f4e";
        let source_error =
            agent_client_protocol::Error::new(-32001, format!("denied {token_sentinel}"))
                .data(serde_json::json!({"account": account_sentinel, "token": token_sentinel}));
        let runtime_error = crate::ExternalProviderRuntimeError::Initialize(
            crate::external_provider_runtime::sanitized_initialization_error(&source_error),
        );

        let reason = provider_startup_failure_reason("/fixture/acp-provider", &runtime_error);

        assert!(reason.contains("initialize"));
        assert!(reason.contains("-32001"));
        assert!(reason.contains("error_data_bytes="));
        assert!(!reason.contains(account_sentinel));
        assert!(!reason.contains(token_sentinel));
    }
}
