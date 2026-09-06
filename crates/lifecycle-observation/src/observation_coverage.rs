//! Disposable observer coverage; persisted status never creates live authority.
use crate::JournalError;
use communication_protocol::{
    BackendStatus, CoverageState, CoverageView, EndpointRef, LifecycleChange, LifecycleObservation,
    ObservationScope, ObservationTimestamp,
};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct ObservationCoverage {
    views: BTreeMap<EndpointRef, CoverageView>,
}
impl ObservationCoverage {
    /// Apply live observation-owner facts only; never replay journal history through this method.
    pub fn apply(&mut self, observation: &LifecycleObservation) -> Result<(), JournalError> {
        observation
            .validate()
            .map_err(|_| JournalError::InvalidRecord)?;
        let state = match &observation.change {
            LifecycleChange::CoverageRestored => CoverageState::Observing,
            LifecycleChange::CoverageLost => CoverageState::Disconnected,
            LifecycleChange::BackendStatus {
                status: BackendStatus::Starting | BackendStatus::Unavailable | BackendStatus::Failed,
            } => CoverageState::Disconnected,
            _ => return Ok(()),
        };
        self.views.insert(
            observation.scope.endpoint.clone(),
            CoverageView {
                endpoint: observation.scope.endpoint.clone(),
                observer_id: Some(observation.scope.observer_id.clone()),
                generation: observation.scope.generation.clone(),
                state,
                observed_at: observation.observed_at.clone(),
            },
        );
        Ok(())
    }
    #[must_use]
    pub fn is_observing(&self, scope: &ObservationScope) -> bool {
        self.views.get(&scope.endpoint).is_some_and(|view| {
            view.state == CoverageState::Observing
                && view.observer_id.as_ref() == Some(&scope.observer_id)
                && view.generation == scope.generation
                && scope.generation.is_some()
        })
    }
    #[must_use]
    pub fn view(&self, endpoint: &EndpointRef, at: ObservationTimestamp) -> CoverageView {
        self.views
            .get(endpoint)
            .cloned()
            .unwrap_or_else(|| CoverageView {
                endpoint: endpoint.clone(),
                observer_id: None,
                generation: None,
                state: CoverageState::Initializing,
                observed_at: at,
            })
    }
    pub fn storage_unavailable(&mut self) {
        for view in self.views.values_mut() {
            view.state = CoverageState::StorageUnavailable;
        }
    }
}
