//! Registry DTOs and validated machine connection names, addresses and references.
use collaboration_client::protocol::UuidIdentity;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum RouterRegistryError {
    #[error("machine registry could not be read")]
    Unreadable,
    #[error("machine registry exceeds its read limit")]
    TooLarge,
    #[error("machine registry is not valid JSONC")]
    InvalidJsonc,
    #[error("machine registry has invalid or unsupported fields")]
    InvalidShape,
    #[error("machine registry version is unsupported")]
    UnsupportedVersion,
    #[error("machine registry contains duplicate object members")]
    DuplicateMembers,
    #[error("machine registry contains duplicate names")]
    DuplicateName,
    #[error("machine registry contains an invalid machine name")]
    InvalidName,
    #[error("machine registry contains an invalid MCP address")]
    InvalidMcpAddress,
    #[error("machine registry contains an invalid native address")]
    InvalidNativeAddress,
    #[error("machine registry contains an invalid credential reference")]
    InvalidCredentialReference,
    #[error("machine registry contains an invalid remote working directory")]
    InvalidRemoteCwd,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct RouterConnectionName(String);

impl RouterConnectionName {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RouterConnectionName {
    type Error = RouterRegistryError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
            return Err(RouterRegistryError::InvalidName);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct McpConnectionAddress(String);

impl TryFrom<String> for McpConnectionAddress {
    type Error = RouterRegistryError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let url = parse_address(&value).ok_or(RouterRegistryError::InvalidMcpAddress)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(RouterRegistryError::InvalidMcpAddress);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeCodexAddress(String);

impl NativeCodexAddress {
    pub(crate) fn allows_credential_reference(&self) -> bool {
        let Ok(url) = Url::parse(self.as_str()) else {
            return false;
        };
        url.scheme() == "wss"
            || (url.scheme() == "ws"
                && match url.host() {
                    Some(url::Host::Ipv4(address)) => address.is_loopback(),
                    Some(url::Host::Ipv6(address)) => address.is_loopback(),
                    Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                    None => false,
                })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NativeCodexAddress {
    type Error = RouterRegistryError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let url = parse_address(&value).ok_or(RouterRegistryError::InvalidNativeAddress)?;
        let authority = value
            .split_once("://")
            .map(|(_, tail)| tail.split('/').next().unwrap_or_default());
        let port = authority
            .and_then(|host| host.rsplit_once(':'))
            .and_then(|(_, port)| port.parse::<u16>().ok());
        if !matches!(url.scheme(), "ws" | "wss")
            || url.path() != "/"
            || !port.is_some_and(|port| port > 0)
        {
            return Err(RouterRegistryError::InvalidNativeAddress);
        }
        // Keep explicit default ports: Url serializes :443/:80 away, but Codex requires them.
        Ok(Self(value))
    }
}

fn parse_address(value: &str) -> Option<Url> {
    if value.trim() != value || value.chars().any(char::is_control) || value.contains('\\') {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialEnvironmentName(String);

impl TryFrom<String> for CredentialEnvironmentName {
    type Error = RouterRegistryError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut characters = value.chars();
        let first_valid = characters
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !first_valid || !characters.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(RouterRegistryError::InvalidCredentialReference);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CredentialReference {
    Environment { variable: CredentialEnvironmentName },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AbsoluteRemoteCwd(String);

impl AbsoluteRemoteCwd {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AbsoluteRemoteCwd {
    type Error = RouterRegistryError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !value.starts_with('/') || value.chars().any(char::is_control) {
            return Err(RouterRegistryError::InvalidRemoteCwd);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeCodexConnection {
    pub(crate) address: NativeCodexAddress,
    pub(crate) credential: Option<CredentialReference>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouterConnectionProfile {
    pub(crate) name: RouterConnectionName,
    pub(crate) service_id: UuidIdentity,
    pub(crate) mcp_address: McpConnectionAddress,
    pub(crate) credential: Option<CredentialReference>,
    pub(crate) native_codex: Option<NativeCodexConnection>,
    pub(crate) default_remote_cwd: Option<AbsoluteRemoteCwd>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RouterRegistryDocument {
    pub(super) version: u32,
    pub(super) routers: Vec<RouterConnectionProfileDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RouterConnectionProfileDto {
    name: String,
    connection: RemoteConnectionDto,
    default_remote_cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum RemoteConnectionDto {
    Remote {
        #[serde(rename = "serviceId")]
        service_id: UuidIdentity,
        #[serde(rename = "mcpUrl")]
        mcp_url: String,
        credential: Option<CredentialReferenceDto>,
        #[serde(rename = "nativeCodex")]
        native_codex: Option<NativeCodexConnectionDto>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeCodexConnectionDto {
    address: String,
    credential: Option<CredentialReferenceDto>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum CredentialReferenceDto {
    Environment { variable: String },
}

impl TryFrom<CredentialReferenceDto> for CredentialReference {
    type Error = RouterRegistryError;
    fn try_from(value: CredentialReferenceDto) -> Result<Self, Self::Error> {
        let CredentialReferenceDto::Environment { variable } = value;
        Ok(Self::Environment {
            variable: variable.try_into()?,
        })
    }
}

impl TryFrom<RouterConnectionProfileDto> for RouterConnectionProfile {
    type Error = RouterRegistryError;
    fn try_from(value: RouterConnectionProfileDto) -> Result<Self, Self::Error> {
        let RemoteConnectionDto::Remote {
            service_id,
            mcp_url,
            credential,
            native_codex,
        } = value.connection;
        Ok(Self {
            name: value.name.try_into()?,
            service_id,
            mcp_address: mcp_url.try_into()?,
            credential: credential.map(TryInto::try_into).transpose()?,
            native_codex: native_codex
                .map(|native| {
                    Ok(NativeCodexConnection {
                        address: native.address.try_into()?,
                        credential: native.credential.map(TryInto::try_into).transpose()?,
                    })
                })
                .transpose()?,
            default_remote_cwd: value
                .default_remote_cwd
                .map(TryInto::try_into)
                .transpose()?,
        })
    }
}
