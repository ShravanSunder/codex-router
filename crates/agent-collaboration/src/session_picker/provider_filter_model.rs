//! Client-provider visibility is transient presentation state, separate from backend queries.
use collaboration_client::protocol::ProviderKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionProviderFamily {
    Codex,
    External(ProviderKind),
    Unclassified,
}

impl SessionProviderFamily {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::External(ProviderKind::ClaudeCode) => "Claude Code",
            Self::External(ProviderKind::Cursor) => "Cursor",
            Self::Unclassified => "Unclassified",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SessionProviderFilter {
    #[default]
    All,
    Codex,
    ClaudeCode,
    Cursor,
}

impl SessionProviderFilter {
    pub(super) const CHOICES: [Self; 4] = [Self::All, Self::Codex, Self::ClaudeCode, Self::Cursor];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::All => "All providers",
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
        }
    }

    pub(super) fn matches(self, family: SessionProviderFamily) -> bool {
        match self {
            Self::All => true,
            Self::Codex => family == SessionProviderFamily::Codex,
            Self::ClaudeCode => family == SessionProviderFamily::External(ProviderKind::ClaudeCode),
            Self::Cursor => family == SessionProviderFamily::External(ProviderKind::Cursor),
        }
    }

    fn shifted(self, direction: isize) -> Self {
        let index = Self::CHOICES
            .iter()
            .position(|choice| *choice == self)
            .unwrap_or(0);
        Self::CHOICES
            .get(
                index
                    .saturating_add_signed(direction)
                    .min(Self::CHOICES.len() - 1),
            )
            .copied()
            .unwrap_or(self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ProviderFilterFocus {
    #[default]
    Provider,
    Apply,
    Cancel,
}

impl ProviderFilterFocus {
    fn shifted(self, reverse: bool) -> Self {
        match (self, reverse) {
            (Self::Provider, false) | (Self::Cancel, true) => Self::Apply,
            (Self::Apply, false) | (Self::Provider, true) => Self::Cancel,
            (Self::Cancel, false) | (Self::Apply, true) => Self::Provider,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProviderFilterDraft {
    pub(super) selection: SessionProviderFilter,
    pub(super) focus: ProviderFilterFocus,
}

impl ProviderFilterDraft {
    pub(super) fn new(selection: SessionProviderFilter) -> Self {
        Self {
            selection,
            focus: ProviderFilterFocus::Provider,
        }
    }

    pub(super) fn move_provider(&mut self, direction: isize) {
        if self.focus == ProviderFilterFocus::Provider {
            self.selection = self.selection.shifted(direction);
        }
    }

    pub(super) fn move_focus(&mut self, reverse: bool) {
        self.focus = self.focus.shifted(reverse);
    }
}
