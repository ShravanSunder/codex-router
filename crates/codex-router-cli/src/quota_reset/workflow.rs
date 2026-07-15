#![allow(dead_code)]

use std::fmt;
use std::future::Future;

use codex_router_core::ids::AccountId;

use super::domain::ActiveCredentialGeneration;
use super::domain::AttemptGeneration;
use super::domain::ConsumePortResult;
use super::domain::CreditInventoryPortResult;
use super::domain::LiveUsagePortResult;
use super::domain::OperationGeneration;
use super::domain::OperationKind;
use super::domain::RedeemRequestId;
use super::domain::RenderSafeFailure;

/// Correlation identity repeated by every request and completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) struct OperationCorrelation {
    account_id: AccountId,
    active_credential_generation: ActiveCredentialGeneration,
    attempt_generation: AttemptGeneration,
    operation_generation: OperationGeneration,
}

impl OperationCorrelation {
    pub(in crate::quota_reset) const fn new(
        account_id: AccountId,
        active_credential_generation: ActiveCredentialGeneration,
        attempt_generation: AttemptGeneration,
        operation_generation: OperationGeneration,
    ) -> Self {
        Self {
            account_id,
            active_credential_generation,
            attempt_generation,
            operation_generation,
        }
    }

    pub(in crate::quota_reset) const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub(in crate::quota_reset) const fn active_credential_generation(
        &self,
    ) -> ActiveCredentialGeneration {
        self.active_credential_generation
    }

    pub(in crate::quota_reset) const fn attempt_generation(&self) -> AttemptGeneration {
        self.attempt_generation
    }

    pub(in crate::quota_reset) const fn operation_generation(&self) -> OperationGeneration {
        self.operation_generation
    }
}

/// Typed workflow effect request whose variant determines its operation kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) enum CorrelatedRequest {
    InspectionLiveUsage(OperationCorrelation),
    InspectionCreditInventory(OperationCorrelation),
    RevalidationLiveUsage(OperationCorrelation),
    RevalidationCreditInventory(OperationCorrelation),
    ConsumeCredit(OperationCorrelation),
}

impl CorrelatedRequest {
    pub(in crate::quota_reset) const fn inspection_live_usage(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::InspectionLiveUsage(correlation)
    }

    pub(in crate::quota_reset) const fn inspection_credit_inventory(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::InspectionCreditInventory(correlation)
    }

    pub(in crate::quota_reset) const fn revalidation_live_usage(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::RevalidationLiveUsage(correlation)
    }

    pub(in crate::quota_reset) const fn revalidation_credit_inventory(
        correlation: OperationCorrelation,
    ) -> Self {
        Self::RevalidationCreditInventory(correlation)
    }

    pub(in crate::quota_reset) const fn consume_credit(correlation: OperationCorrelation) -> Self {
        Self::ConsumeCredit(correlation)
    }

    pub(in crate::quota_reset) const fn correlation(&self) -> &OperationCorrelation {
        match self {
            Self::InspectionLiveUsage(correlation)
            | Self::InspectionCreditInventory(correlation)
            | Self::RevalidationLiveUsage(correlation)
            | Self::RevalidationCreditInventory(correlation)
            | Self::ConsumeCredit(correlation) => correlation,
        }
    }

    pub(in crate::quota_reset) const fn operation_kind(&self) -> OperationKind {
        match self {
            Self::InspectionLiveUsage(_) => OperationKind::InspectionLiveUsage,
            Self::InspectionCreditInventory(_) => OperationKind::InspectionCreditInventory,
            Self::RevalidationLiveUsage(_) => OperationKind::RevalidationLiveUsage,
            Self::RevalidationCreditInventory(_) => OperationKind::RevalidationCreditInventory,
            Self::ConsumeCredit(_) => OperationKind::ConsumeCredit,
        }
    }
}

/// Typed workflow completion whose variant constrains its terminal category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::quota_reset) enum CorrelatedOutcome {
    InspectionLiveUsage {
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    },
    InspectionCreditInventory {
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    },
    RevalidationLiveUsage {
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    },
    RevalidationCreditInventory {
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    },
    ConsumeCredit {
        correlation: OperationCorrelation,
        terminal: ConsumePortResult,
    },
}

impl CorrelatedOutcome {
    pub(in crate::quota_reset) const fn inspection_live_usage(
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    ) -> Self {
        Self::InspectionLiveUsage {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn inspection_credit_inventory(
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    ) -> Self {
        Self::InspectionCreditInventory {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn revalidation_live_usage(
        correlation: OperationCorrelation,
        terminal: LiveUsagePortResult,
    ) -> Self {
        Self::RevalidationLiveUsage {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn revalidation_credit_inventory(
        correlation: OperationCorrelation,
        terminal: CreditInventoryPortResult,
    ) -> Self {
        Self::RevalidationCreditInventory {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn consume_credit(
        correlation: OperationCorrelation,
        terminal: ConsumePortResult,
    ) -> Self {
        Self::ConsumeCredit {
            correlation,
            terminal,
        }
    }

    pub(in crate::quota_reset) const fn correlation(&self) -> &OperationCorrelation {
        match self {
            Self::InspectionLiveUsage { correlation, .. }
            | Self::InspectionCreditInventory { correlation, .. }
            | Self::RevalidationLiveUsage { correlation, .. }
            | Self::RevalidationCreditInventory { correlation, .. }
            | Self::ConsumeCredit { correlation, .. } => correlation,
        }
    }

    pub(in crate::quota_reset) const fn operation_kind(&self) -> OperationKind {
        match self {
            Self::InspectionLiveUsage { .. } => OperationKind::InspectionLiveUsage,
            Self::InspectionCreditInventory { .. } => OperationKind::InspectionCreditInventory,
            Self::RevalidationLiveUsage { .. } => OperationKind::RevalidationLiveUsage,
            Self::RevalidationCreditInventory { .. } => OperationKind::RevalidationCreditInventory,
            Self::ConsumeCredit { .. } => OperationKind::ConsumeCredit,
        }
    }

    pub(in crate::quota_reset) const fn live_usage_terminal(&self) -> Option<&LiveUsagePortResult> {
        match self {
            Self::InspectionLiveUsage { terminal, .. }
            | Self::RevalidationLiveUsage { terminal, .. } => Some(terminal),
            Self::InspectionCreditInventory { .. }
            | Self::RevalidationCreditInventory { .. }
            | Self::ConsumeCredit { .. } => None,
        }
    }
}

/// Render-safe workflow phase without authority-bearing values.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkflowPhase {
    #[default]
    Browse,
    Inspecting,
    Inspected,
    Confirming,
    Revalidating,
    Committing,
    Result,
}

/// Explicit provenance for values that may be shown in reset detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenderValueProvenance {
    CurrentLive,
    PreviousLiveRefreshing,
    Saved,
}

/// Render-safe semantic activity state for one provider operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationActivity<TValue> {
    NotStarted,
    Loading,
    Refreshing {
        previous: Option<TValue>,
    },
    Succeeded(TValue),
    Failed {
        failure: RenderSafeFailure,
        previous: Option<TValue>,
    },
    Cancelled,
    RequestDispatchedAwaitingOutcome,
}

struct ConfirmationAuthority;
struct PinnedRoutingAuthority;
struct SelectedCreditAuthority;

struct CommitAuthority {
    account_id: AccountId,
    active_credential_generation: ActiveCredentialGeneration,
    attempt_generation: AttemptGeneration,
    confirmation: ConfirmationAuthority,
    pinned_routing: PinnedRoutingAuthority,
    selected_credit: SelectedCreditAuthority,
    redeem_request_id: RedeemRequestId,
}

/// Single-owner authority required to cross the provider consume boundary.
pub(in crate::quota_reset) struct CommitCapability {
    authority: CommitAuthority,
}

impl fmt::Debug for CommitCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommitCapability(<opaque>)")
    }
}

/// By-value consume boundary. Implementations cannot invoke consume without authority ownership.
pub(in crate::quota_reset) trait CommitCapabilityConsumer {
    fn consume(
        &self,
        capability: CommitCapability,
    ) -> impl Future<Output = ConsumePortResult> + Send;
}

#[cfg(test)]
mod tests {
    use codex_router_core::ids::AccountId;

    use super::super::domain::CreditInventorySummary;
    use super::super::domain::KnownConsumeOutcome;
    use super::super::domain::LiveWeeklyUsage;
    use super::*;

    #[test]
    fn operation_kinds_are_the_five_visible_provider_operations() {
        assert_eq!(
            OperationKind::ALL,
            [
                OperationKind::InspectionLiveUsage,
                OperationKind::InspectionCreditInventory,
                OperationKind::RevalidationLiveUsage,
                OperationKind::RevalidationCreditInventory,
                OperationKind::ConsumeCredit,
            ]
        );
    }

    #[test]
    fn request_and_outcome_repeat_all_correlation_fields() {
        let correlation = OperationCorrelation::new(
            AccountId::new("account-a")
                .unwrap_or_else(|error| panic!("account id should be valid: {error}")),
            ActiveCredentialGeneration::new(7),
            AttemptGeneration::new(11),
            OperationGeneration::new(13),
        );
        let requests = [
            CorrelatedRequest::inspection_live_usage(correlation.clone()),
            CorrelatedRequest::inspection_credit_inventory(correlation.clone()),
            CorrelatedRequest::revalidation_live_usage(correlation.clone()),
            CorrelatedRequest::revalidation_credit_inventory(correlation.clone()),
            CorrelatedRequest::consume_credit(correlation.clone()),
        ];
        let outcomes = [
            CorrelatedOutcome::inspection_live_usage(
                correlation.clone(),
                LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
            ),
            CorrelatedOutcome::inspection_credit_inventory(
                correlation.clone(),
                CreditInventoryPortResult::Validated(CreditInventorySummary {
                    credit_count: 1,
                    usable_credit_count: 1,
                }),
            ),
            CorrelatedOutcome::revalidation_live_usage(
                correlation.clone(),
                LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)),
            ),
            CorrelatedOutcome::revalidation_credit_inventory(
                correlation.clone(),
                CreditInventoryPortResult::Validated(CreditInventorySummary {
                    credit_count: 1,
                    usable_credit_count: 1,
                }),
            ),
            CorrelatedOutcome::consume_credit(
                correlation.clone(),
                ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed),
            ),
        ];

        for ((request, outcome), expected_kind) in
            requests.iter().zip(outcomes.iter()).zip(OperationKind::ALL)
        {
            assert_eq!(request.operation_kind(), expected_kind);
            assert_eq!(outcome.operation_kind(), expected_kind);
            assert_eq!(request.correlation(), &correlation);
            assert_eq!(outcome.correlation(), &correlation);
        }
        assert_eq!(correlation.active_credential_generation().get(), 7);
        assert_eq!(correlation.attempt_generation().get(), 11);
        assert_eq!(correlation.operation_generation().get(), 13);
        assert_eq!(
            outcomes[0].live_usage_terminal(),
            Some(&LiveUsagePortResult::Known(LiveWeeklyUsage::new(0)))
        );
    }

    #[test]
    fn activity_and_consume_results_have_only_render_safe_categories() {
        let previous = LiveWeeklyUsage::new(0);
        let activities = [
            OperationActivity::NotStarted,
            OperationActivity::Loading,
            OperationActivity::Refreshing {
                previous: Some(previous),
            },
            OperationActivity::Succeeded(previous),
            OperationActivity::Failed {
                failure: RenderSafeFailure::Transport,
                previous: Some(previous),
            },
            OperationActivity::Cancelled,
            OperationActivity::RequestDispatchedAwaitingOutcome,
        ];

        assert_eq!(activities.len(), 7);
        assert_eq!(WorkflowPhase::default(), WorkflowPhase::Browse);
        assert_ne!(
            RenderValueProvenance::Saved,
            RenderValueProvenance::CurrentLive
        );
        assert_eq!(
            CreditInventoryPortResult::Validated(CreditInventorySummary {
                credit_count: 3,
                usable_credit_count: 1,
            }),
            CreditInventoryPortResult::Validated(CreditInventorySummary {
                credit_count: 3,
                usable_credit_count: 1,
            })
        );
        assert_eq!(
            ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed),
            ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed)
        );
        assert!(matches!(
            ConsumePortResult::OutcomeUnknown(RenderSafeFailure::Transport),
            ConsumePortResult::OutcomeUnknown(_)
        ));
        assert_eq!(
            RenderSafeFailure::TimedOut.message(),
            "provider operation timed out"
        );
        let source = include_str!("workflow.rs");
        assert!(!source.contains(&["RenderSafeFailure", "::new"].concat()));
        let domain_source = include_str!("domain.rs");
        let failure_contract = domain_source
            .split("pub(crate) enum RenderSafeFailure")
            .nth(1)
            .and_then(|source| source.split("/// Validated live weekly usage").next())
            .unwrap_or_else(|| panic!("render-safe failure contract should exist"));
        assert!(!failure_contract.contains("String"));
        assert!(!failure_contract.contains("fn new"));
        let redeem_contract = domain_source
            .split("struct RedeemRequestId")
            .nth(1)
            .and_then(|source| source.split("/// The five provider operations").next())
            .unwrap_or_else(|| panic!("redeem request identity contract should exist"));
        assert!(!redeem_contract.contains("fn new"));
        assert!(!redeem_contract.contains("as_str"));
    }

    #[test]
    fn commit_capability_source_contract_is_opaque_and_by_value() {
        let source = include_str!("workflow.rs");
        let capability_start = source
            .find("pub(crate) struct CommitCapability")
            .unwrap_or_else(|| panic!("commit capability declaration should exist"));
        let capability_source = source
            .get(capability_start..)
            .unwrap_or_else(|| panic!("capability source range should exist"));
        let declaration_prefix = source
            .get(..capability_start)
            .unwrap_or_else(|| panic!("capability declaration prefix should exist"));
        let preceding_declaration = declaration_prefix
            .get(declaration_prefix.len().saturating_sub(160)..)
            .unwrap_or(declaration_prefix);
        let capability_body = capability_source
            .split("impl fmt::Debug for CommitCapability")
            .next()
            .unwrap_or(capability_source);
        let authority_body = source
            .split("struct CommitAuthority")
            .nth(1)
            .and_then(|source| source.split("/// Single-owner authority").next())
            .unwrap_or_else(|| panic!("commit authority declaration should exist"));

        assert!(!preceding_declaration.contains(&["derive", "(Clone"].concat()));
        assert!(!preceding_declaration.contains("Serialize"));
        assert!(!capability_source.contains(&["impl Clone", " for CommitCapability"].concat()));
        assert!(!capability_source.contains(&["Serialize", " for CommitCapability"].concat()));
        assert!(!capability_body.contains("pub(crate) authority"));
        assert!(!authority_body.contains("String"));
        assert!(source.contains("pub(in crate::quota_reset) struct CommitCapability"));
        assert!(source.contains("capability: CommitCapability"));
        assert!(!source.contains(&["capability: ", "&CommitCapability"].concat()));
    }
}
