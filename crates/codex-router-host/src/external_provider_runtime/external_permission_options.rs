//! Keep supported ACP permission choices usable when a provider adds an unknown choice.

use agent_client_protocol::schema::v1::{PermissionOption, PermissionOptionKind};
use collaboration_service::{ExternalApprovalOption, ExternalApprovalOptionScope};
use std::collections::BTreeSet;

pub(super) enum ExternalPermissionOptionMapping {
    Mapped(Vec<ExternalApprovalOption>),
    Refused {
        reason: &'static str,
        options: Vec<ExternalApprovalOption>,
    },
}

#[derive(Clone)]
enum ExternalPermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unsupported { provider_kind: String },
}

pub(super) fn map_external_permission_options(
    options: Vec<PermissionOption>,
) -> ExternalPermissionOptionMapping {
    map_classified_options(options.into_iter().map(|option| {
        let kind = match option.kind {
            PermissionOptionKind::AllowOnce => ExternalPermissionKind::AllowOnce,
            PermissionOptionKind::AllowAlways => ExternalPermissionKind::AllowAlways,
            PermissionOptionKind::RejectOnce => ExternalPermissionKind::RejectOnce,
            PermissionOptionKind::RejectAlways => ExternalPermissionKind::RejectAlways,
            kind => ExternalPermissionKind::Unsupported {
                provider_kind: format!("{kind:?}"),
            },
        };
        (
            option.option_id.0.to_string(),
            bounded_option_label(&option.name),
            kind,
        )
    }))
}

fn map_classified_options(
    options: impl IntoIterator<Item = (String, Option<String>, ExternalPermissionKind)>,
) -> ExternalPermissionOptionMapping {
    let mut mapped = Vec::new();
    let mut has_allow_once = false;
    let mut saw_unsupported_kind = false;
    let mut identifiers = BTreeSet::new();
    let mut decision_kinds = BTreeSet::new();
    let mut malformed_options = None;

    for (option_id, label, kind) in options {
        if !identifiers.insert(option_id.clone()) {
            malformed_options = Some("permission options contain a duplicate identifier");
        }
        let one_time_decision = match kind {
            ExternalPermissionKind::AllowOnce => Some("allowOnce"),
            ExternalPermissionKind::RejectOnce => Some("rejectOnce"),
            ExternalPermissionKind::AllowAlways
            | ExternalPermissionKind::RejectAlways
            | ExternalPermissionKind::Unsupported { .. } => None,
        };
        if one_time_decision.is_some_and(|decision| !decision_kinds.insert(decision)) {
            malformed_options = Some("permission options contain duplicate one-time decisions");
        }
        let scope = match kind {
            ExternalPermissionKind::AllowOnce => {
                has_allow_once = true;
                ExternalApprovalOptionScope::AllowOnce
            }
            ExternalPermissionKind::AllowAlways => ExternalApprovalOptionScope::AllowAlways,
            ExternalPermissionKind::RejectOnce => ExternalApprovalOptionScope::RejectOnce,
            ExternalPermissionKind::RejectAlways => ExternalApprovalOptionScope::RejectAlways,
            ExternalPermissionKind::Unsupported { provider_kind } => {
                saw_unsupported_kind = true;
                ExternalApprovalOptionScope::Unsupported { provider_kind }
            }
        };
        mapped.push(ExternalApprovalOption {
            option_id,
            label,
            scope,
        });
    }

    if let Some(reason) = malformed_options {
        ExternalPermissionOptionMapping::Refused {
            reason,
            options: mapped,
        }
    } else if has_allow_once {
        ExternalPermissionOptionMapping::Mapped(mapped)
    } else if saw_unsupported_kind {
        ExternalPermissionOptionMapping::Refused {
            reason: "no supported allow option remains after ignoring an unsupported permission option kind",
            options: mapped,
        }
    } else {
        ExternalPermissionOptionMapping::Refused {
            reason: "no one-time allow option is available",
            options: mapped,
        }
    }
}

fn bounded_option_label(label: &str) -> Option<String> {
    Some(label.chars().take(120).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_option_kind_does_not_discard_a_mappable_allow() {
        let mapping = map_classified_options(vec![
            (
                "allow".to_owned(),
                Some("Allow".to_owned()),
                ExternalPermissionKind::AllowOnce,
            ),
            (
                "unknown".to_owned(),
                Some("Continue".to_owned()),
                ExternalPermissionKind::Unsupported {
                    provider_kind: "Future".to_owned(),
                },
            ),
            (
                "deny".to_owned(),
                Some("Reject".to_owned()),
                ExternalPermissionKind::RejectOnce,
            ),
        ]);

        let ExternalPermissionOptionMapping::Mapped(options) = mapping else {
            panic!("mappable allow option should be retained");
        };
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].scope, ExternalApprovalOptionScope::AllowOnce);
        assert_eq!(
            options[1].scope,
            ExternalApprovalOptionScope::Unsupported {
                provider_kind: "Future".to_owned()
            }
        );
        assert_eq!(options[2].scope, ExternalApprovalOptionScope::RejectOnce);
    }

    #[test]
    fn no_allow_option_returns_a_visible_refusal_reason() {
        let mapping = map_classified_options(vec![(
            "deny".to_owned(),
            Some("Reject".to_owned()),
            ExternalPermissionKind::RejectOnce,
        )]);

        let ExternalPermissionOptionMapping::Refused { reason, .. } = mapping else {
            panic!("deny-only options must fail closed");
        };
        assert_eq!(reason, "no one-time allow option is available");
    }

    #[test]
    fn unknown_option_without_a_supported_allow_is_refused_with_reason() {
        let mapping = map_classified_options(vec![
            (
                "deny".to_owned(),
                Some("Reject".to_owned()),
                ExternalPermissionKind::RejectOnce,
            ),
            (
                "future".to_owned(),
                Some("Future".to_owned()),
                ExternalPermissionKind::Unsupported {
                    provider_kind: "Future".to_owned(),
                },
            ),
        ]);

        let ExternalPermissionOptionMapping::Refused { reason, .. } = mapping else {
            panic!("unknown kind without a supported allow must fail closed");
        };
        assert_eq!(
            reason,
            "no supported allow option remains after ignoring an unsupported permission option kind"
        );
    }
}
