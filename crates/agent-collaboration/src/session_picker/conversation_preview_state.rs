//! Source-scoped preview loading and publication for one picker view.
use std::collections::BTreeMap;

use crate::sessions::{
    SessionConversationPreview, SessionConversationSource, SessionPickerIdentity,
    SessionPickerRecord,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ConversationPreviewKey {
    pub(super) identity: SessionPickerIdentity,
    pub(super) source: SessionConversationSource,
    pub(super) generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ConversationPreviewLoadRequest {
    pub(super) key: ConversationPreviewKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ConversationPreviewLoadState {
    Loading,
    Loaded(SessionConversationPreview),
}

struct ConversationPreviewEntry {
    key: ConversationPreviewKey,
    state: ConversationPreviewLoadState,
}

#[derive(Default)]
pub(super) struct ConversationPreviewCache {
    generation: u64,
    // A source change replaces the identity's slot; key equality also checks its home and view.
    entries: BTreeMap<SessionPickerIdentity, ConversationPreviewEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SelectedConversationPreview {
    pub(super) preview: SessionConversationPreview,
    pub(super) load_request: Option<ConversationPreviewLoadRequest>,
}

impl ConversationPreviewCache {
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.entries.clear();
    }

    pub(super) fn select_record(
        &self,
        record: &SessionPickerRecord,
    ) -> SelectedConversationPreview {
        let Some(source) = record.conversation_source.as_ref() else {
            return SelectedConversationPreview {
                preview: record.conversation.clone(),
                load_request: None,
            };
        };
        let key = ConversationPreviewKey {
            identity: record.identity.clone(),
            source: source.clone(),
            generation: self.generation,
        };
        let entry = self
            .entries
            .get(&key.identity)
            .filter(|entry| entry.key == key);
        match entry.map(|entry| &entry.state) {
            Some(ConversationPreviewLoadState::Loaded(preview)) => SelectedConversationPreview {
                preview: preview.clone(),
                load_request: None,
            },
            Some(ConversationPreviewLoadState::Loading) => SelectedConversationPreview {
                preview: record.conversation.clone(),
                load_request: None,
            },
            None => SelectedConversationPreview {
                preview: record.conversation.clone(),
                load_request: Some(ConversationPreviewLoadRequest { key }),
            },
        }
    }

    pub(super) fn start_loading(&mut self, request: &ConversationPreviewLoadRequest) {
        if request.key.generation != self.generation {
            return;
        }
        self.entries.insert(
            request.key.identity.clone(),
            ConversationPreviewEntry {
                key: request.key.clone(),
                state: ConversationPreviewLoadState::Loading,
            },
        );
    }

    pub(super) fn complete_load(
        &mut self,
        request: &ConversationPreviewLoadRequest,
        preview: SessionConversationPreview,
    ) {
        if request.key.generation != self.generation {
            return;
        }
        if let Some(entry) = self.entries.get_mut(&request.key.identity)
            && entry.key == request.key
            && matches!(entry.state, ConversationPreviewLoadState::Loading)
        {
            entry.state = ConversationPreviewLoadState::Loaded(preview);
        }
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
