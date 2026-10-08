//! The lifecycle journal and the address book: long-polled lifecycle reads and paged address
//! snapshots for the endpoints this Router publishes.
use super::{CollaborationRejection, CollaborationRejectionReason, SessionOperations};
use collaboration_protocol::{AddressListParams, JournalReadParams, JournalStatus};
use lifecycle_observation::{AddressPage, JournalBounds, JournalError, JournalPage};
use serde_json::json;

impl<'service> SessionOperations<'service> {
    /// The lifecycle journal's bounds, or `unavailable` when this Router keeps no journal.
    pub async fn journal_status(&self) -> JournalStatus {
        match self.identity.journal.as_ref() {
            Some(store) => match store.bounds().await {
                Ok(bounds) => JournalStatus::Available { bounds },
                Err(_) => JournalStatus::Unavailable,
            },
            None => JournalStatus::Unavailable,
        }
    }

    /// Reads one endpoint's lifecycle records after `after`, waiting up to `waitMilliseconds`.
    pub async fn journal_read(
        &self,
        request: JournalReadParams,
    ) -> Result<JournalPage, JournalFailure> {
        if !(1..=100).contains(&request.page_size)
            || request.wait_milliseconds > 30000
            || request.after.sequence > 9_007_199_254_740_991
        {
            return Err(JournalFailure::InvalidRequest);
        }
        let store = self.local_journal(&request.endpoint)?;
        match store
            .read_wait(
                &request.endpoint,
                request.after,
                request.page_size,
                request.wait_milliseconds,
            )
            .await
        {
            Ok(page) => Ok(page),
            Err(JournalError::InvalidRecord) => Err(JournalFailure::InvalidRequest),
            Err(error @ (JournalError::HistoryExpired | JournalError::JournalChanged)) => {
                let cause = if matches!(error, JournalError::HistoryExpired) {
                    JournalCursorInvalidation::HistoryExpired
                } else {
                    JournalCursorInvalidation::JournalChanged
                };
                match store.bounds().await {
                    Ok(current) => Err(JournalFailure::CursorInvalidated { cause, current }),
                    Err(_) => Err(JournalFailure::Unavailable(
                        JournalUnavailableKind::Unavailable,
                    )),
                }
            }
            Err(_) => Err(JournalFailure::Unavailable(
                JournalUnavailableKind::Unavailable,
            )),
        }
    }

    /// Pages one endpoint's address book from a snapshot taken on the first page.
    pub async fn address_list(
        &self,
        request: AddressListParams,
    ) -> Result<AddressPage, JournalFailure> {
        if !(1..=100).contains(&request.page_size) {
            return Err(JournalFailure::InvalidRequest);
        }
        let store = self.local_journal(&request.endpoint)?;
        let unavailable = JournalFailure::Unavailable(JournalUnavailableKind::Unavailable);
        let Ok(snapshot_id) = crate::new_service_uuid() else {
            return Err(unavailable);
        };
        let Ok(at) = collaboration_protocol::ObservationTimestamp::try_from(
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ) else {
            return Err(unavailable);
        };
        let Ok(page_size) = usize::try_from(request.page_size) else {
            return Err(JournalFailure::InvalidRequest);
        };
        store
            .address_snapshot(
                &request.endpoint,
                page_size,
                request.cursor.as_deref(),
                snapshot_id,
                at,
            )
            .await
            .map_err(|error| match error {
                JournalError::SnapshotExpired => JournalFailure::SnapshotExpired,
                JournalError::InvalidRecord => JournalFailure::InvalidRequest,
                JournalError::Capacity => {
                    JournalFailure::Unavailable(JournalUnavailableKind::Overloaded)
                }
                _ => unavailable,
            })
    }

    /// The journal for an endpoint this Router publishes.
    fn local_journal(
        &self,
        endpoint: &collaboration_protocol::EndpointRef,
    ) -> Result<&'service lifecycle_observation::LifecycleStore, JournalFailure> {
        if endpoint.service_id != self.identity.service_id {
            return Err(JournalFailure::Unavailable(
                JournalUnavailableKind::WrongService,
            ));
        }
        match self.identity.directory.read_endpoint(endpoint) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(JournalFailure::Unavailable(
                    JournalUnavailableKind::EndpointNotFound,
                ));
            }
            Err(_) => {
                return Err(JournalFailure::Unavailable(
                    JournalUnavailableKind::Unavailable,
                ));
            }
        }
        self.identity
            .journal
            .as_deref()
            .ok_or(JournalFailure::Unavailable(
                JournalUnavailableKind::Unavailable,
            ))
    }
}

/// Why a lifecycle journal or address-book read failed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum JournalFailure {
    #[error("Invalid lifecycle parameters")]
    InvalidRequest,
    /// `{kind, stage: discovery, message}`.
    #[error("Lifecycle read unavailable")]
    Unavailable(JournalUnavailableKind),
    /// The address snapshot behind the cursor was evicted; start a fresh listing.
    #[error("Snapshot expired")]
    SnapshotExpired,
    /// The cursor no longer names retained history; `current` gives the journal's bounds.
    #[error("Journal cursor invalidated")]
    CursorInvalidated {
        cause: JournalCursorInvalidation,
        current: JournalBounds,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JournalUnavailableKind {
    WrongService,
    EndpointNotFound,
    Unavailable,
    /// The address snapshot cache is full.
    Overloaded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JournalCursorInvalidation {
    HistoryExpired,
    JournalChanged,
}

impl serde::Serialize for JournalFailure {
    fn serialize<TSerializer: serde::Serializer>(
        &self,
        serializer: TSerializer,
    ) -> Result<TSerializer::Ok, TSerializer::Error> {
        let payload = match self {
            Self::InvalidRequest => json!({"kind":"invalidRequest","message":self.to_string()}),
            Self::Unavailable(kind) => {
                json!({"kind":kind,"stage":"discovery","message":self.to_string()})
            }
            Self::SnapshotExpired => json!({"kind":"snapshotExpired"}),
            Self::CursorInvalidated { cause, current } => json!({"kind":cause,"current":current}),
        };
        payload.serialize(serializer)
    }
}

impl CollaborationRejection for JournalFailure {
    fn rejection_reason(&self) -> Option<CollaborationRejectionReason> {
        match self {
            Self::InvalidRequest => Some(CollaborationRejectionReason::InvalidShape),
            Self::Unavailable(kind) => match kind {
                JournalUnavailableKind::Overloaded => {
                    Some(CollaborationRejectionReason::Overloaded)
                }
                JournalUnavailableKind::WrongService
                | JournalUnavailableKind::EndpointNotFound
                | JournalUnavailableKind::Unavailable => None,
            },
            Self::SnapshotExpired | Self::CursorInvalidated { .. } => None,
        }
    }
}
