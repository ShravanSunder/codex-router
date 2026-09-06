//! Bounded paging of captured historical rows, without retaining database transactions.
use crate::{CapturedAddresses, CoverageView, JournalError};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use communication_protocol::{AddressPage, ObservationTimestamp};
use communication_protocol::{EndpointRef, UuidIdentity};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct AddressSnapshotCache {
    snapshots: BTreeMap<UuidIdentity, RetainedSnapshot>,
}
struct RetainedSnapshot {
    endpoint: EndpointRef,
    page_size: usize,
    captured: CapturedAddresses,
    expires: Instant,
    coverage: CoverageView,
    captured_at: ObservationTimestamp,
}
impl AddressSnapshotCache {
    pub fn insert(&mut self, inputs: AddressSnapshotInputs) -> Result<AddressPage, JournalError> {
        let AddressSnapshotInputs {
            id,
            endpoint,
            page_size,
            captured,
            now,
            coverage,
            captured_at,
        } = inputs;
        self.snapshots.retain(|_, snapshot| snapshot.expires > now);
        if !(1..=100).contains(&page_size) || self.snapshots.contains_key(&id) {
            return Err(JournalError::InvalidRecord);
        }
        let bytes: usize = self
            .snapshots
            .values()
            .map(|snapshot| snapshot.captured.serialized_bytes)
            .sum();
        if self.snapshots.len() >= 16
            || bytes.saturating_add(captured.serialized_bytes) > 32 * 1024 * 1024
        {
            return Err(JournalError::Capacity);
        }
        let snapshot = RetainedSnapshot {
            endpoint,
            page_size,
            captured,
            expires: now + Duration::from_secs(60),
            coverage,
            captured_at,
        };
        let page = make_page(&id, &snapshot, 0)?;
        self.snapshots.insert(id, snapshot);
        Ok(page)
    }
    pub fn page(
        &mut self,
        endpoint: &EndpointRef,
        page_size: usize,
        cursor: &str,
        now: Instant,
    ) -> Result<AddressPage, JournalError> {
        if cursor.is_empty() || cursor.len() > 1024 {
            return Err(JournalError::InvalidRecord);
        }
        let decoded = URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| JournalError::InvalidRecord)?;
        let text = String::from_utf8(decoded).map_err(|_| JournalError::InvalidRecord)?;
        let (id, offset) = text.split_once(':').ok_or(JournalError::InvalidRecord)?;
        let id = UuidIdentity::try_from(id.to_owned()).map_err(|_| JournalError::InvalidRecord)?;
        let offset: usize = offset.parse().map_err(|_| JournalError::InvalidRecord)?;
        self.snapshots.retain(|_, snapshot| snapshot.expires > now);
        let snapshot = self
            .snapshots
            .get(&id)
            .ok_or(JournalError::SnapshotExpired)?;
        if snapshot.endpoint != *endpoint
            || snapshot.page_size != page_size
            || offset == 0
            || offset >= snapshot.captured.entries.len()
        {
            return Err(JournalError::InvalidRecord);
        }
        make_page(&id, snapshot, offset)
    }
}
fn make_page(
    id: &UuidIdentity,
    snapshot: &RetainedSnapshot,
    offset: usize,
) -> Result<AddressPage, JournalError> {
    let mut entries = Vec::new();
    let mut bytes = 0_usize;
    for entry in snapshot
        .captured
        .entries
        .iter()
        .skip(offset)
        .take(snapshot.page_size)
    {
        let size = serde_json::to_vec(entry)
            .map_err(|_| JournalError::InvalidStorage)?
            .len();
        if size > 1024 * 1024 - 4096 {
            return Err(JournalError::Capacity);
        }
        if bytes + size > 1024 * 1024 - 4096 {
            break;
        }
        bytes += size;
        entries.push(entry.clone());
    }
    let next = offset + entries.len();
    Ok(AddressPage {
        snapshot_id: id.clone(),
        captured_at: snapshot.captured_at.clone(),
        coverage: snapshot.coverage.clone(),
        watermark: snapshot.captured.watermark.clone(),
        entries,
        next_cursor: (next < snapshot.captured.entries.len())
            .then(|| URL_SAFE_NO_PAD.encode(format!("{}:{next}", String::from(id.clone())))),
    })
}

pub struct AddressSnapshotInputs {
    pub id: UuidIdentity,
    pub endpoint: EndpointRef,
    pub page_size: usize,
    pub captured: CapturedAddresses,
    pub now: Instant,
    pub coverage: CoverageView,
    pub captured_at: ObservationTimestamp,
}
