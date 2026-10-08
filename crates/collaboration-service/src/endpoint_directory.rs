//! Atomic endpoint snapshots: the Host publishes endpoints and readers take the inventory.
use collaboration_protocol::{EndpointDescription, EndpointRef, UuidIdentity};
use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct EndpointDirectory {
    state: Arc<Mutex<DirectoryState>>,
}
struct DirectoryState {
    service_id: UuidIdentity,
    endpoints: BTreeMap<EndpointRef, EndpointDescription>,
    /// Counts every publication across the Host, independent of any subscriber.
    publications: u64,
}
pub struct EndpointSnapshot {
    pub sequence: u64,
    pub endpoints: Vec<EndpointDescription>,
}
impl EndpointDirectory {
    pub fn read_endpoint(&self, endpoint: &EndpointRef) -> io::Result<Option<EndpointDescription>> {
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("endpoint directory unavailable"))?;
        Ok(state.endpoints.get(endpoint).cloned())
    }

    #[must_use]
    pub fn new(service_id: UuidIdentity) -> Self {
        Self {
            state: Arc::new(Mutex::new(DirectoryState {
                service_id,
                endpoints: BTreeMap::new(),
                publications: 0,
            })),
        }
    }
    pub fn publish(&self, endpoint: EndpointDescription) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("endpoint directory unavailable"))?;
        if endpoint.endpoint.service_id != state.service_id
            || endpoint.channels.len() > 2
            || (endpoint.channels.is_empty()
                && !matches!(
                    endpoint.availability,
                    collaboration_protocol::EndpointAvailability::Unavailable { .. }
                ))
        {
            return Err(io::Error::other("invalid endpoint registration"));
        }
        if state.endpoints.len() >= 64 && !state.endpoints.contains_key(&endpoint.endpoint) {
            return Err(io::Error::other("endpoint capacity exceeded"));
        }
        state.endpoints.insert(endpoint.endpoint.clone(), endpoint);
        state.publications = state.publications.saturating_add(1);
        Ok(())
    }
    /// Every published endpoint, with the Host-wide count of publications they reflect.
    pub fn inventory(&self) -> io::Result<EndpointSnapshot> {
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("endpoint directory unavailable"))?;
        Ok(EndpointSnapshot {
            sequence: state.publications,
            endpoints: state.endpoints.values().cloned().collect(),
        })
    }
    /// A reader of the current inventory, with no change notifications. The Codex
    /// app-server delivery route (a harness file that stays byte-identical) reads the
    /// inventory through it; new readers call [`Self::inventory`].
    pub fn subscribe(&self) -> io::Result<EndpointInventoryReader> {
        Ok(EndpointInventoryReader {
            directory: self.clone(),
        })
    }
}

/// Reads the endpoint inventory on demand; see [`EndpointDirectory::subscribe`].
pub struct EndpointInventoryReader {
    directory: EndpointDirectory,
}

impl EndpointInventoryReader {
    pub fn snapshot(&self) -> io::Result<EndpointSnapshot> {
        self.directory.inventory()
    }
}
