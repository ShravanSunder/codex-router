//! Provider-owned destination disclosed before a persistent permission choice.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProviderPersistenceTarget {
    CursorAllowlist,
    ClaudeSettingsRule,
    #[default]
    Unspecified,
}

impl ProviderPersistenceTarget {
    pub const fn disclosure(self) -> &'static str {
        match self {
            Self::CursorAllowlist => {
                "Cursor allowlist (Cursor decides whether global or per-project)"
            }
            Self::ClaudeSettingsRule => "Claude Code permission rule in its settings",
            Self::Unspecified => "the agent's own persistent permissions (location not reported)",
        }
    }
}
