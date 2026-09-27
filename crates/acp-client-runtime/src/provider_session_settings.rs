//! Typed provider Session settings at the ACP client boundary.

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestedProviderSettings {
    pub mode: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl RequestedProviderSettings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mode.is_none() && self.model.is_none() && self.effort.is_none()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectiveProviderSettings {
    pub mode: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSettingKind {
    Mode,
    Model,
    Effort,
}

impl ProviderSettingKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Model => "model",
            Self::Effort => "effort",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedProviderSetting {
    pub kind: ProviderSettingKind,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedProviderSetting {
    pub kind: ProviderSettingKind,
    pub value: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidSettingSessionDisposition {
    Closed,
    RemainsCreated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSettingChoice {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderConfigValue {
    Select(String),
    Boolean(bool),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderConfigOption {
    pub id: String,
    pub name: String,
    pub category: Option<ProviderSettingKind>,
    pub current_value: ProviderConfigValue,
    pub choices: Vec<ProviderSettingChoice>,
}

/// Last options advertised by a provider Session. The Host supervisor may keep
/// this snapshot after the Session unloads for model/list and settings inspect.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderSettingsCatalog {
    pub current_mode: Option<String>,
    pub modes: Vec<ProviderSettingChoice>,
    pub config_options: Vec<ProviderConfigOption>,
}

impl ProviderSettingsCatalog {
    #[must_use]
    pub fn advertised_values(&self, kind: ProviderSettingKind) -> Vec<String> {
        if kind == ProviderSettingKind::Mode
            && !self
                .config_options
                .iter()
                .any(|option| option.category == Some(kind))
        {
            return self
                .modes
                .iter()
                .map(|choice| choice.value.clone())
                .collect();
        }
        self.config_options
            .iter()
            .find(|option| option.category == Some(kind))
            .map(|option| {
                option
                    .choices
                    .iter()
                    .map(|choice| choice.value.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[must_use]
    pub fn config_option(&self, kind: ProviderSettingKind) -> Option<&ProviderConfigOption> {
        self.config_options
            .iter()
            .find(|option| option.category == Some(kind))
    }

    #[must_use]
    pub fn effective_settings(&self) -> EffectiveProviderSettings {
        let selected = |kind| {
            self.config_option(kind)
                .and_then(|option| match &option.current_value {
                    ProviderConfigValue::Select(value) => Some(value.clone()),
                    ProviderConfigValue::Boolean(_) => None,
                })
        };
        EffectiveProviderSettings {
            mode: selected(ProviderSettingKind::Mode).or_else(|| self.current_mode.clone()),
            model: selected(ProviderSettingKind::Model),
            effort: selected(ProviderSettingKind::Effort),
        }
    }
}
