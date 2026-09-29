//! Nonblocking display names learned from existing Router session observations.
use collaboration_protocol::{
    SessionDisplayName, SessionDisplayNameLookup, SessionDisplayNameLookupError, SessionRef,
};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock, TryLockError},
};

const MAX_CACHED_SESSION_DISPLAY_NAMES: usize = 4096;

#[derive(Clone, Default)]
pub struct SessionDisplayNameCache {
    names: Arc<RwLock<HashMap<SessionRef, SessionDisplayName>>>,
}

impl SessionDisplayNameCache {
    /// Remembers a display name, or removes a stale one when the name was cleared/invalid.
    ///
    /// Cache contention is logged and never changes the outcome of the operation that
    /// learned the name.
    pub fn remember(&self, session: SessionRef, name: &str) {
        let Ok(name) = SessionDisplayName::try_from(name.to_owned()) else {
            self.forget(session);
            return;
        };
        let mut names = match self.names.try_write() {
            Ok(names) => names,
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
                tracing::warn!(session = ?session, "session display name cache write was contended");
                return;
            }
        };
        if names.len() >= MAX_CACHED_SESSION_DISPLAY_NAMES
            && !names.contains_key(&session)
            && let Some(evicted_session) = names.keys().next().cloned()
        {
            names.remove(&evicted_session);
        }
        names.insert(session, name);
    }

    /// Removes a cached label after the Router observes that its name was cleared.
    pub fn forget(&self, session: SessionRef) {
        match self.names.try_write() {
            Ok(mut names) => {
                names.remove(&session);
            }
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
                tracing::warn!(session = ?session, "session display name cache clear was contended");
            }
        }
    }
}

impl SessionDisplayNameLookup for SessionDisplayNameCache {
    fn display_name_for(
        &self,
        session: &SessionRef,
    ) -> Result<Option<SessionDisplayName>, SessionDisplayNameLookupError> {
        match self.names.try_read() {
            Ok(names) => Ok(names.get(session).cloned()),
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => {
                Err(SessionDisplayNameLookupError::Unavailable)
            }
        }
    }
}

impl std::fmt::Debug for SessionDisplayNameCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionDisplayNameCache")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use collaboration_protocol::{EndpointId, EndpointRef, SessionId, UuidIdentity};

    fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
        SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned(),
                )
                .expect("valid service UUID"),
                endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).expect("valid endpoint"),
            },
            session_id: SessionId::try_from(session_id.to_owned()).expect("valid session"),
        }
    }

    #[test]
    fn cache_returns_the_last_valid_name_for_an_exact_session_reference() {
        let cache = SessionDisplayNameCache::default();
        let session_ref = session("codex-local", "same-id");
        let other_endpoint = session("claude-local", "same-id");

        cache.remember(session_ref.clone(), "Initial name");
        cache.remember(session_ref.clone(), "Renamed session");

        assert_eq!(
            cache
                .display_name_for(&session_ref)
                .expect("cache lookup")
                .as_ref()
                .map(SessionDisplayName::as_str),
            Some("Renamed session")
        );
        assert_eq!(cache.display_name_for(&other_endpoint), Ok(None));
    }

    #[test]
    fn a_cleared_or_invalid_name_removes_the_previous_cached_name() {
        let cache = SessionDisplayNameCache::default();
        let session_ref = session("codex-local", "named-session");
        cache.remember(session_ref.clone(), "Codex Main");
        cache.remember(session_ref.clone(), "Name → forged header");

        assert_eq!(cache.display_name_for(&session_ref), Ok(None));
    }
}
