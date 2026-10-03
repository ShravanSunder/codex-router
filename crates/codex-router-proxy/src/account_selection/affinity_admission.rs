use super::*;

pub(super) fn session_id_for_route(
    request: &HttpProxyRequest,
    route_kind: RouteKind,
) -> Option<&str> {
    let header_name = if route_kind == RouteKind::ClaudeMessages {
        "x-claude-code-session-id"
    } else if route_kind.previous_response_affinity_capable() {
        "session-id"
    } else {
        return None;
    };
    request
        .header_value(header_name)
        .filter(|session_id| !session_id.is_empty())
}

pub(super) fn session_affinity_lookup_session_id(
    session_id: Option<&str>,
    previous_response_owner_resolved: bool,
) -> Option<&str> {
    if previous_response_owner_resolved {
        return None;
    }
    session_id
}

pub(super) fn account_id_from_affinity_owner_lookup(
    owner_lookup: PreviousResponseAffinityOwnerLookup,
) -> Result<AccountId, HttpProxyError> {
    match owner_lookup {
        PreviousResponseAffinityOwnerLookup::Found(owner) => Ok(owner.account_id().clone()),
        PreviousResponseAffinityOwnerLookup::Missing => Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing,
        }),
        PreviousResponseAffinityOwnerLookup::Ambiguous => Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
        }),
    }
}

pub(super) fn select_affinity_owner(
    route_band: RouteBand,
    provider: Provider,
    owner_account_id: &AccountId,
    assessment: &BurnDownRouteBandAssessmentResult,
    account_holds: &mut HashMap<ProviderRouteBand, AccountHold>,
    now_unix_seconds: u64,
    selection_reason: &'static str,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let selection_scope = ProviderRouteBand::new(provider, route_band);
    if !assessment_account_is_available(assessment, owner_account_id) {
        account_holds.remove(&selection_scope);
        return Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
        });
    }

    account_holds.insert(
        selection_scope,
        AccountHold::new(owner_account_id.clone(), now_unix_seconds),
    );
    Ok(
        SelectedAccountDecision::new(owner_account_id.clone(), selection_reason)
            .with_credit_backed_at_selection(assessment_account_is_credit_backed(
                assessment,
                owner_account_id,
            )),
    )
}

pub(super) fn assessment_account_is_available(
    assessment: &BurnDownRouteBandAssessmentResult,
    account_id: &AccountId,
) -> bool {
    assessment.accounts().iter().any(|account| {
        account.account_id() == account_id
            && matches!(
                account.availability(),
                AccountAvailability::Usable | AccountAvailability::Reserve
            )
            && (account.quota_evidence_reason() != QuotaEvidenceReason::CreditBacked
                || assessment
                    .weighted_candidates()
                    .iter()
                    .any(|(candidate_id, _)| candidate_id == account_id))
    })
}

pub(super) fn assessment_account_is_credit_backed(
    assessment: &BurnDownRouteBandAssessmentResult,
    account_id: &AccountId,
) -> bool {
    assessment.accounts().iter().any(|account| {
        account.account_id() == account_id
            && account.quota_evidence_reason() == QuotaEvidenceReason::CreditBacked
            && account.routing_reason() == RoutingReason::CreditBacked
    })
}

pub(super) fn assessment_account_must_yield(
    assessment: &BurnDownRouteBandAssessmentResult,
    account_id: &AccountId,
    route_profile: &RouteProfile,
) -> bool {
    assessment.accounts().iter().any(|account| {
        if account.account_id() != account_id {
            return false;
        }
        if route_profile.provider == Provider::Claude {
            return assessment.selected_pool() == SelectedPool::Usable
                && account.availability() == AccountAvailability::Reserve;
        }
        account.routing_reason() == RoutingReason::HeldFloorSwitch
    })
}

pub(super) fn publish_selected_session_affinity(
    selected: SelectedAccountDecision,
    provider: Provider,
    cache: &SharedSessionAccountAffinityCache,
    writer: Option<&DbWriteActor>,
    session_id: Option<&str>,
    route_band: RouteBand,
    now_unix_seconds: u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    if provider == Provider::Claude {
        return Ok(selected);
    }
    let Some(session_id) = session_id else {
        return Ok(selected);
    };

    let published = publish_session_account_affinity(
        cache,
        provider,
        session_id,
        selected.account_id(),
        route_band,
        writer,
        now_unix_seconds,
    )
    .map_err(|_error| HttpProxyError::Selection {
        reason: QuotaAwareAccountSelectorError::StateUnavailable,
    })?;
    Ok(selected.with_session_affinity_activity_handle(published.activity_handle().clone()))
}
