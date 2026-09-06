//! Atomic endpoint snapshots and per-subscriber bounded publication ordering.
use communication_protocol::{EndpointDescription, EndpointRef, UuidIdentity};
use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct EndpointDirectory {
    state: Arc<Mutex<DirectoryState>>,
}
struct DirectoryState {
    service_id: UuidIdentity,
    endpoints: BTreeMap<EndpointRef, EndpointDescription>,
    subscribers: BTreeMap<u64, SubscriberState>,
    next_subscriber: u64,
}
struct SubscriberState {
    sequence: u64,
    sender: mpsc::Sender<EndpointUpdate>,
    overflow: Arc<AtomicBool>,
}
#[derive(Clone, Debug)]
pub struct EndpointUpdate {
    pub sequence: u64,
    pub endpoint: EndpointDescription,
}
pub struct EndpointSnapshot {
    pub sequence: u64,
    pub endpoints: Vec<EndpointDescription>,
}
pub struct EndpointSubscription {
    directory: EndpointDirectory,
    id: u64,
    receiver: mpsc::Receiver<EndpointUpdate>,
    overflow: Arc<AtomicBool>,
}
impl EndpointDirectory {
    #[must_use]
    pub fn new(service_id: UuidIdentity) -> Self {
        Self {
            state: Arc::new(Mutex::new(DirectoryState {
                service_id,
                endpoints: BTreeMap::new(),
                subscribers: BTreeMap::new(),
                next_subscriber: 0,
            })),
        }
    }
    pub fn publish(&self, endpoint: EndpointDescription) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("endpoint directory unavailable"))?;
        if endpoint.endpoint.service_id != state.service_id
            || !(1..=2).contains(&endpoint.channels.len())
        {
            return Err(io::Error::other("invalid endpoint registration"));
        }
        if state.endpoints.len() >= 64 && !state.endpoints.contains_key(&endpoint.endpoint) {
            return Err(io::Error::other("endpoint capacity exceeded"));
        }
        state
            .endpoints
            .insert(endpoint.endpoint.clone(), endpoint.clone());
        state.subscribers.retain(|_, subscriber| {
            if subscriber.sequence >= 9_007_199_254_740_991 {
                subscriber.overflow.store(true, Ordering::Release);
                return false;
            }
            let sequence = subscriber.sequence + 1;
            match subscriber.sender.try_send(EndpointUpdate {
                sequence,
                endpoint: endpoint.clone(),
            }) {
                Ok(()) => {
                    subscriber.sequence = sequence;
                    true
                }
                Err(_) => {
                    subscriber.overflow.store(true, Ordering::Release);
                    false
                }
            }
        });
        Ok(())
    }
    pub fn subscribe(&self) -> io::Result<EndpointSubscription> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("endpoint directory unavailable"))?;
        if state.subscribers.len() >= 32 {
            return Err(io::Error::other("connection capacity exceeded"));
        }
        let id = state.next_subscriber;
        state.next_subscriber = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("subscriber identity exhausted"))?;
        let (sender, receiver) = mpsc::channel(1024);
        let overflow = Arc::new(AtomicBool::new(false));
        state.subscribers.insert(
            id,
            SubscriberState {
                sequence: 0,
                sender,
                overflow: Arc::clone(&overflow),
            },
        );
        Ok(EndpointSubscription {
            directory: self.clone(),
            id,
            receiver,
            overflow,
        })
    }
}
impl EndpointSubscription {
    pub fn snapshot(&self) -> io::Result<EndpointSnapshot> {
        let state = self
            .directory
            .state
            .lock()
            .map_err(|_| io::Error::other("endpoint directory unavailable"))?;
        let subscriber = state
            .subscribers
            .get(&self.id)
            .ok_or_else(|| io::Error::other("subscription closed"))?;
        Ok(EndpointSnapshot {
            sequence: subscriber.sequence,
            endpoints: state.endpoints.values().cloned().collect(),
        })
    }
    pub async fn next(&mut self) -> io::Result<EndpointUpdate> {
        if self.overflow.load(Ordering::Acquire) {
            return Err(io::Error::other("subscription overflow"));
        }
        let next = self
            .receiver
            .recv()
            .await
            .ok_or_else(|| io::Error::other("subscription closed"))?;
        if self.overflow.load(Ordering::Acquire) {
            return Err(io::Error::other("subscription overflow"));
        }
        Ok(next)
    }
}
impl Drop for EndpointSubscription {
    fn drop(&mut self) {
        if let Ok(mut state) = self.directory.state.lock() {
            state.subscribers.remove(&self.id);
        }
    }
}
