//! Upstream provider identities shared by account and routing state.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// Provider whose OAuth accounts Router can select.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// OpenAI Codex accounts.
    Openai,
    /// Anthropic Claude subscription accounts.
    Claude,
}

impl Provider {
    /// Returns the stable persisted provider name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Claude => "claude",
        }
    }

    /// Parses a stable provider name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai" => Some(Self::Openai),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Provider;

    #[test]
    fn provider_names_are_stable_and_unknown_values_are_rejected() {
        let providers = [(Provider::Openai, "openai"), (Provider::Claude, "claude")];

        for (provider, expected_name) in providers {
            assert_eq!(provider.as_str(), expected_name);
            assert_eq!(Provider::parse(expected_name), Some(provider));
        }

        assert_eq!(Provider::parse("OPENAI"), None);
        assert_eq!(Provider::parse("unknown"), None);
    }
}
