//! Decode ACP option kinds into ordered, exact-ID approval choices.

use std::collections::BTreeSet;

use agent_client_protocol::schema::v1::{PermissionOption, PermissionOptionKind};
use session_event_model::{
    ApprovalChoice, ApprovalEffect, ApprovalScope, OfferedOption, OfferedOptionId, OfferedOptions,
};

use crate::ProviderPersistenceTarget;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefusedPermissionOption {
    pub option_id: String,
    pub label: String,
    pub provider_kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefusedPermissionOptions {
    pub reason: &'static str,
    pub options: Vec<RefusedPermissionOption>,
}

pub(crate) fn map_permission_options(
    options: Vec<PermissionOption>,
    persistent_target: ProviderPersistenceTarget,
) -> Result<OfferedOptions, RefusedPermissionOptions> {
    let original_options = options
        .iter()
        .map(|option| RefusedPermissionOption {
            option_id: option.option_id.0.to_string(),
            label: bounded_option_label(&option.name),
            provider_kind: format!("{:?}", option.kind),
        })
        .collect::<Vec<_>>();
    let refusal = |reason| RefusedPermissionOptions {
        reason,
        options: original_options.clone(),
    };
    if options.is_empty() {
        return Err(refusal("permission options are empty"));
    }

    let mut identifiers = BTreeSet::new();
    let mut mapped = Vec::with_capacity(options.len());
    for option in options {
        let option_id = option.option_id.0.to_string();
        if option_id.is_empty() {
            return Err(refusal("permission option identifier is empty"));
        }
        if !identifiers.insert(option_id.clone()) {
            return Err(refusal("permission options contain a duplicate identifier"));
        }
        let choice = match option.kind {
            PermissionOptionKind::AllowOnce => {
                ApprovalChoice::new(ApprovalEffect::Allow, ApprovalScope::Once)
            }
            PermissionOptionKind::RejectOnce => {
                ApprovalChoice::new(ApprovalEffect::Decline, ApprovalScope::Once)
            }
            PermissionOptionKind::AllowAlways => ApprovalChoice::new(
                ApprovalEffect::Allow,
                ApprovalScope::persistent(persistent_target.disclosure())
                    .map_err(|_| refusal("persistent permission destination is unavailable"))?,
            ),
            PermissionOptionKind::RejectAlways => ApprovalChoice::new(
                ApprovalEffect::Decline,
                ApprovalScope::persistent(persistent_target.disclosure())
                    .map_err(|_| refusal("persistent permission destination is unavailable"))?,
            ),
            _ => return Err(refusal("permission option kind is unrecognized")),
        };
        let option_id = OfferedOptionId::new(option_id)
            .map_err(|_| refusal("permission option identifier is empty"))?;
        mapped.push(OfferedOption {
            option_id,
            label: bounded_option_label(&option.name),
            choice,
        });
    }
    OfferedOptions::new(mapped)
        .map_err(|_| refusal("permission options are empty or contain duplicate identifiers"))
}

fn bounded_option_label(label: &str) -> String {
    label.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oracle: specification R17 requires persistent scope and disclosure before selection.
    #[test]
    fn persistent_options_keep_effect_order_and_provider_destination() {
        let options = vec![
            PermissionOption::new("always", "Always allow", PermissionOptionKind::AllowAlways),
            PermissionOption::new("never", "Always reject", PermissionOptionKind::RejectAlways),
        ];
        for (target, disclosure) in [
            (
                ProviderPersistenceTarget::CursorAllowlist,
                "Cursor allowlist (Cursor decides whether global or per-project)",
            ),
            (
                ProviderPersistenceTarget::ClaudeSettingsRule,
                "Claude Code permission rule in its settings",
            ),
            (
                ProviderPersistenceTarget::Unspecified,
                "the agent's own persistent permissions (location not reported)",
            ),
        ] {
            let mapped = map_permission_options(options.clone(), target)
                .expect("persistent-only offer is valid");
            let ordered: Vec<_> = mapped.iter().collect();
            assert_eq!(ordered[0].option_id.as_str(), "always");
            assert_eq!(ordered[0].choice.effect, ApprovalEffect::Allow);
            assert_eq!(ordered[1].option_id.as_str(), "never");
            assert_eq!(ordered[1].choice.effect, ApprovalEffect::Decline);
            for option in ordered {
                let ApprovalScope::Persistent { where_stored } = &option.choice.scope else {
                    panic!("always choice must be persistent");
                };
                assert_eq!(where_stored.as_str(), disclosure);
            }
        }
    }

    /// Oracle: specification R17 permits every offered persistent choice;
    /// there is no requirement for an allow-once companion.
    #[test]
    fn allow_always_only_is_a_normal_offer() {
        let mapped = map_permission_options(
            vec![PermissionOption::new(
                "allow-always",
                "Always allow",
                PermissionOptionKind::AllowAlways,
            )],
            ProviderPersistenceTarget::CursorAllowlist,
        )
        .expect("allow-always alone is selectable");
        assert_eq!(mapped.iter().count(), 1);
    }

    /// Oracle: specification E10 requires one offered choice per exact option ID.
    #[test]
    fn duplicate_option_id_is_refused_with_original_order() {
        let refused = map_permission_options(
            vec![
                PermissionOption::new("same", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("same", "Reject", PermissionOptionKind::RejectOnce),
            ],
            ProviderPersistenceTarget::Unspecified,
        )
        .expect_err("duplicate IDs are malformed");
        assert_eq!(
            refused.reason,
            "permission options contain a duplicate identifier"
        );
        assert_eq!(refused.options.len(), 2);
        assert_eq!(refused.options[0].label, "Allow");
        assert_eq!(refused.options[1].label, "Reject");
    }

    #[test]
    fn unclassifiable_kind_is_refused() {
        // An empty set is the parser-independent malformed-offer case.
        let refused = map_permission_options(Vec::new(), ProviderPersistenceTarget::Unspecified)
            .expect_err("no choice can be offered");
        assert_eq!(refused.reason, "permission options are empty");
    }
}
