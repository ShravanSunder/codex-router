//! One source-owned request tuple and view-specific opaque continuation.
use collaboration_client::protocol::{
    CodexGeneration, NativeSessionListParams, NativeSessionListResult, NativeSessionObservation,
    NativeSessionView, SessionRef,
};
use collaboration_client::{ClientError, ControlClient};
use std::collections::BTreeSet;

const MAX_INVENTORY_ROWS: usize = 4096;
// The existing default reader allows 64 continuation cursors plus the final page.
const MAX_INVENTORY_PAGES: usize = 65;

enum InventoryObservationContext {
    Stored,
    Runtime(CodexGeneration),
}

enum NativePagingState {
    Ready,
    Complete,
    Rejected,
}

pub(super) struct NativeInventoryPager {
    request: NativeSessionListParams,
    observation_context: InventoryObservationContext,
    seen_cursors: BTreeSet<String>,
    seen_targets: BTreeSet<SessionRef>,
    pages_read: usize,
    paging_state: NativePagingState,
}

impl NativeInventoryPager {
    pub(super) fn new(
        request: NativeSessionListParams,
        expected_generation: Option<CodexGeneration>,
    ) -> Result<Self, ClientError> {
        if !(1..=100).contains(&request.page_size) {
            return Err(ClientError::InvalidRequest("invalid session page size"));
        }
        let observation_context = match (request.view, expected_generation) {
            (NativeSessionView::Stored, None) => InventoryObservationContext::Stored,
            (NativeSessionView::Loaded | NativeSessionView::Active, Some(generation)) => {
                InventoryObservationContext::Runtime(generation)
            }
            _ => {
                return Err(ClientError::InvalidRequest(
                    "inventory view and generation disagree",
                ));
            }
        };
        let mut seen_cursors = BTreeSet::new();
        if let Some(cursor) = &request.cursor {
            validate_cursor(cursor)?;
            seen_cursors.insert(cursor.clone());
        }
        Ok(Self {
            request,
            observation_context,
            seen_cursors,
            seen_targets: BTreeSet::new(),
            pages_read: 0,
            paging_state: NativePagingState::Ready,
        })
    }

    pub(super) async fn next_page(
        &mut self,
        client: &mut ControlClient,
    ) -> Result<Option<NativeSessionListResult>, ClientError> {
        match self.paging_state {
            NativePagingState::Ready => {}
            NativePagingState::Complete => return Ok(None),
            NativePagingState::Rejected => {
                return Err(ClientError::Protocol("inventory read has already failed"));
            }
        }
        self.paging_state = NativePagingState::Rejected;
        if self.pages_read >= MAX_INVENTORY_PAGES {
            return Err(ClientError::Protocol("inventory pages exceeded bounds"));
        }
        // A failed read/validation cannot be retried through this pager.
        let page = client.list_sessions(self.request.clone()).await?;
        match &self.observation_context {
            InventoryObservationContext::Stored => {
                if page.generation.is_some()
                    || page.sessions.iter().any(|summary| {
                        !matches!(summary.observation, NativeSessionObservation::Stored { .. })
                    })
                {
                    return Err(ClientError::Protocol(
                        "expected stored inventory observation",
                    ));
                }
            }
            InventoryObservationContext::Runtime(expected) => {
                if page.generation.as_ref() != Some(expected) {
                    return Err(ClientError::Protocol(
                        "runtime inventory generation changed",
                    ));
                }
                if page.sessions.iter().any(|summary| {
                    !matches!(
                        summary.observation,
                        NativeSessionObservation::Runtime { .. }
                    )
                }) {
                    return Err(ClientError::Protocol("expected runtime observation"));
                }
            }
        }
        for summary in &page.sessions {
            if self.seen_targets.len() >= MAX_INVENTORY_ROWS
                || !self.seen_targets.insert(summary.target.clone())
            {
                return Err(ClientError::Protocol("inventory rows exceeded bounds"));
            }
        }
        if let Some(next_cursor) = &page.next_cursor {
            validate_cursor(next_cursor)?;
            if !self.seen_cursors.insert(next_cursor.clone()) || self.seen_cursors.len() > 64 {
                return Err(ClientError::Protocol(
                    "inventory continuation did not converge",
                ));
            }
            self.request.cursor = Some(next_cursor.clone());
            self.paging_state = NativePagingState::Ready;
        } else {
            self.paging_state = NativePagingState::Complete;
        }
        self.pages_read += 1;
        Ok(Some(page))
    }
}

fn validate_cursor(cursor: &str) -> Result<(), ClientError> {
    // Match the existing service's opaque cursor byte bound, without parsing or normalizing it.
    if cursor.is_empty() || cursor.len() > 1024 {
        return Err(ClientError::Protocol("invalid inventory continuation"));
    }
    Ok(())
}
