//! Shared observation of the Router executable at its launch path.

use serde::{Deserialize, Serialize};

/// Running Router Host build compared with the executable at its original launch path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouterExecutableRelation {
    /// The launch path still has its startup file identity or reports the same version.
    Match,
    /// The launch path now resolves to a different Router version or is missing.
    Drift {
        /// Version of the executable captured when the Host started.
        running_version: String,
        /// Version currently reported at the original launch path, if readable.
        installed_version: Option<String>,
    },
    /// The launch path could not be compared reliably.
    Unknown {
        /// Safe, actionable-free reason for the failed observation.
        reason: String,
    },
}

/// Formats the operator warning shared by Host logs, status output, and MCP initialize.
#[must_use]
pub fn router_build_warning(relation: &RouterExecutableRelation) -> Option<String> {
    let RouterExecutableRelation::Drift {
        running_version,
        installed_version,
    } = relation
    else {
        return None;
    };
    Some(format!(
        "⚠ Router Host is stale (running {running_version}, installed {}); run `codex-router host restart`",
        installed_version.as_deref().unwrap_or("unknown")
    ))
}

#[cfg(test)]
mod tests {
    use super::{RouterExecutableRelation, router_build_warning};

    #[test]
    fn drift_warning_includes_versions_and_restart_action() {
        let relation = RouterExecutableRelation::Drift {
            running_version: "0.1.36".to_owned(),
            installed_version: Some("0.1.37".to_owned()),
        };
        assert_eq!(
            router_build_warning(&relation).as_deref(),
            Some(
                "⚠ Router Host is stale (running 0.1.36, installed 0.1.37); run `codex-router host restart`"
            )
        );
    }

    #[test]
    fn match_and_unknown_have_no_drift_warning() {
        assert_eq!(router_build_warning(&RouterExecutableRelation::Match), None);
        assert_eq!(
            router_build_warning(&RouterExecutableRelation::Unknown {
                reason: "path unavailable".to_owned(),
            }),
            None
        );
    }
}
