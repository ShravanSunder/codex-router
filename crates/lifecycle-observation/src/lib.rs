//! Durable lifecycle metadata, separate from native history and message delivery.
mod observation_journal;
pub use observation_journal::{JournalError, JournalRow, ObservationJournal};

mod thread_address_book;
pub use communication_protocol::{
    AddressEntry, ArchiveState, Existence, LifecycleDisposition, StatusOrdering,
};
mod address_book_rebuild;
mod journal_retention;

mod journal_page_reader;
pub use journal_page_reader::{JournalBounds, JournalPage, LifecycleRecord};

mod lifecycle_store;
pub use lifecycle_store::LifecycleStore;

pub use communication_protocol::JournalPosition;
mod address_snapshot_reader;
pub use address_snapshot_reader::CapturedAddresses;

mod address_snapshot_cache;
pub use address_snapshot_cache::{AddressSnapshotCache, AddressSnapshotInputs};
pub use communication_protocol::{AddressPage, CoverageState, CoverageView};
mod observation_coverage;
pub use observation_coverage::ObservationCoverage;
mod native_lifecycle_mapping;
pub use native_lifecycle_mapping::map_native_lifecycle;
mod inventory_reconciliation;
pub use inventory_reconciliation::{InventoryReconciliation, ReadDisposition, ReadTicket};
mod native_observation_stream;
pub use native_observation_stream::{NativeObservationInputs, NativeObservationStream};
