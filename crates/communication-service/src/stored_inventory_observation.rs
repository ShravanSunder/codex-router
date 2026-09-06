//! Record catalog discovery without asserting backend generation or live runtime status.
use communication_protocol::{
    LifecycleChange, LifecycleObservation, LifecycleSubject, NativeSessionListResult,
    ObservationScope, ObservationSource, ThreadAddress, UuidIdentity,
};
use lifecycle_observation::{JournalError, LifecycleStore};

pub(crate) struct StoredInventoryObservation<'a> {
    pub store: &'a LifecycleStore,
    pub observer_id: &'a UuidIdentity,
}
impl StoredInventoryObservation<'_> {
    pub async fn record_page(&self, page: &NativeSessionListResult) -> Result<(), JournalError> {
        for session in &page.sessions {
            self.store
                .append(
                    &LifecycleObservation {
                        observed_at: page.observed_at.clone(),
                        source: ObservationSource::InventoryRead,
                        scope: ObservationScope {
                            endpoint: page.endpoint.clone(),
                            generation: None,
                            observer_id: self.observer_id.clone(),
                        },
                        subject: LifecycleSubject::Thread {
                            address: ThreadAddress {
                                endpoint: page.endpoint.clone(),
                                native_thread_id: session.target.session_id.clone(),
                            },
                        },
                        change: LifecycleChange::ThreadDiscovered,
                    },
                    chrono::Utc::now().timestamp(),
                )
                .await?;
        }
        Ok(())
    }
}
