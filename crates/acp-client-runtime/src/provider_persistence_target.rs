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
            Self::ClaudeSettingsRule => {
                "May add a Claude Code permission rule to its local settings, or apply only to this Session; Claude chooses and does not report which."
            }
            Self::Unspecified => "the agent's own persistent permissions (location not reported)",
        }
    }
}
