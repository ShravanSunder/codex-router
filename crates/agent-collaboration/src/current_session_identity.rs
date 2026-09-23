//! Resolves the calling agent's own session from the harness that runs its tools.
//!
//! Each supported harness exports its conversation ID into tool processes. The shared
//! MCP server never sees that environment, so the CLI is the only place Router can
//! learn "who am I" without the caller declaring it by hand.
use collaboration_client::protocol::{EndpointRef, SessionRef, UuidIdentity};
use std::ffi::OsString;

/// One harness whose tool environment names the current conversation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionHarness {
    pub environment_variable: &'static str,
    pub endpoint_id: &'static str,
}

pub(crate) const SESSION_HARNESSES: [SessionHarness; 3] = [
    SessionHarness {
        environment_variable: "CODEX_THREAD_ID",
        endpoint_id: "codex-local",
    },
    SessionHarness {
        environment_variable: "CLAUDE_CODE_SESSION_ID",
        endpoint_id: "claude-local",
    },
    SessionHarness {
        environment_variable: "CURSOR_CONVERSATION_ID",
        endpoint_id: "cursor-local",
    },
];

/// The harness-reported session, before it is bound to a Router service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HarnessSessionIdentity {
    pub harness: SessionHarness,
    pub session_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CurrentSessionIdentityError {
    Missing,
    Ambiguous { variables: Vec<&'static str> },
    NotUnicode { variable: &'static str },
}

impl std::fmt::Display for CurrentSessionIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let supported = SESSION_HARNESSES
            .iter()
            .map(|harness| harness.environment_variable)
            .collect::<Vec<_>>()
            .join(", ");
        match self {
            Self::Missing => write!(
                formatter,
                "current session identity unavailable: set exactly one of {supported}"
            ),
            Self::Ambiguous { variables } => write!(
                formatter,
                "current session identity is ambiguous: {} are set together; pass an explicit SessionRef",
                variables.join(" and ")
            ),
            Self::NotUnicode { variable } => {
                write!(
                    formatter,
                    "{variable} must contain a non-empty session ID without NUL or invalid Unicode"
                )
            }
        }
    }
}

/// Reads the process environment of the calling agent's tool invocation.
pub(crate) fn read_harness_session_identity()
-> Result<HarnessSessionIdentity, CurrentSessionIdentityError> {
    resolve_harness_session_identity(|name| std::env::var_os(name))
}

/// Exactly one harness variable must be set; nested harnesses are refused, not guessed.
pub(crate) fn resolve_harness_session_identity(
    read_variable: impl Fn(&str) -> Option<OsString>,
) -> Result<HarnessSessionIdentity, CurrentSessionIdentityError> {
    let present = SESSION_HARNESSES
        .iter()
        .filter_map(|harness| {
            read_variable(harness.environment_variable)
                .filter(|value| !value.is_empty())
                .map(|value| (*harness, value))
        })
        .collect::<Vec<_>>();
    match present.as_slice() {
        [] => Err(CurrentSessionIdentityError::Missing),
        [(harness, value)] => {
            let session_id = value.clone().into_string().map_err(|_| {
                CurrentSessionIdentityError::NotUnicode {
                    variable: harness.environment_variable,
                }
            })?;
            Ok(HarnessSessionIdentity {
                harness: *harness,
                session_id,
            })
        }
        several => Err(CurrentSessionIdentityError::Ambiguous {
            variables: several
                .iter()
                .map(|(harness, _)| harness.environment_variable)
                .collect(),
        }),
    }
}

impl HarnessSessionIdentity {
    /// Binds the harness session to the Router service that owns its endpoint.
    pub(crate) fn session_ref(&self, service_id: &UuidIdentity) -> Result<SessionRef, String> {
        Ok(SessionRef {
            endpoint: EndpointRef {
                service_id: service_id.clone(),
                endpoint_id: self
                    .harness
                    .endpoint_id
                    .to_owned()
                    .try_into()
                    .map_err(|_| "invalid endpoint".to_owned())?,
            },
            session_id: self.session_id.clone().try_into().map_err(|_| {
                format!(
                    "{} must contain a non-empty session ID without NUL",
                    self.harness.environment_variable
                )
            })?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CurrentSessionIdentityError, resolve_harness_session_identity};
    use std::{collections::HashMap, ffi::OsString};

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let values = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(*value)))
            .collect::<HashMap<_, _>>();
        move |name| values.get(name).cloned()
    }

    #[test]
    fn each_harness_variable_maps_to_its_local_endpoint() {
        for (variable, endpoint_id) in [
            ("CODEX_THREAD_ID", "codex-local"),
            ("CLAUDE_CODE_SESSION_ID", "claude-local"),
            ("CURSOR_CONVERSATION_ID", "cursor-local"),
        ] {
            // Arrange
            let read = environment(&[(variable, "session-1")]);

            // Act
            let identity = resolve_harness_session_identity(read).expect("single harness");

            // Assert
            assert_eq!(identity.harness.endpoint_id, endpoint_id);
            assert_eq!(identity.session_id, "session-1");
        }
    }

    #[test]
    fn empty_values_count_as_absent() {
        let read = environment(&[("CODEX_THREAD_ID", ""), ("CURSOR_CONVERSATION_ID", "c-1")]);

        let identity = resolve_harness_session_identity(read).expect("empty ignored");

        assert_eq!(identity.harness.endpoint_id, "cursor-local");
    }

    #[test]
    fn missing_and_nested_harnesses_are_refused() {
        assert_eq!(
            resolve_harness_session_identity(environment(&[])),
            Err(CurrentSessionIdentityError::Missing)
        );
        assert_eq!(
            resolve_harness_session_identity(environment(&[
                ("CODEX_THREAD_ID", "t-1"),
                ("CURSOR_CONVERSATION_ID", "c-1"),
            ])),
            Err(CurrentSessionIdentityError::Ambiguous {
                variables: vec!["CODEX_THREAD_ID", "CURSOR_CONVERSATION_ID"],
            })
        );
    }
}
