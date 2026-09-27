//! Project agent-advertised model choices for one provider connection.

use acp_client_runtime::{ProviderSettingKind, ProviderSettingsCatalog};
use collaboration_service::ProviderModelEntry;

pub(crate) fn provider_default_model() -> Result<ProviderModelEntry, &'static str> {
    ProviderModelEntry::try_new(
        "provider-default".into(),
        "provider default".into(),
        "The provider selects its default model".into(),
    )
    .map_err(|_| "invalid provider default model")
}

pub(crate) fn model_entries(catalog: &ProviderSettingsCatalog) -> Vec<ProviderModelEntry> {
    let advertised = catalog
        .config_options
        .iter()
        .find(|option| option.category == Some(ProviderSettingKind::Model))
        .map(|option| option.choices.as_slice())
        .unwrap_or_default();
    let entries = advertised
        .iter()
        .filter_map(|choice| {
            ProviderModelEntry::try_new(
                choice.value.clone(),
                choice.label.clone(),
                "Advertised by the provider".into(),
            )
            .ok()
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        provider_default_model().into_iter().collect()
    } else {
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client_runtime::{ProviderConfigOption, ProviderConfigValue, ProviderSettingChoice};

    #[test]
    fn model_choices_replace_provider_default_in_agent_order() {
        let catalog = ProviderSettingsCatalog {
            current_mode: None,
            modes: Vec::new(),
            config_options: vec![ProviderConfigOption {
                id: "model".into(),
                name: "Model".into(),
                category: Some(ProviderSettingKind::Model),
                current_value: ProviderConfigValue::Select("model-b".into()),
                choices: vec![
                    ProviderSettingChoice {
                        value: "model-a".into(),
                        label: "Model A".into(),
                    },
                    ProviderSettingChoice {
                        value: "model-b".into(),
                        label: "Model B".into(),
                    },
                ],
            }],
        };
        let entries = model_entries(&catalog);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            ProviderModelEntry::try_new(
                "model-a".into(),
                "Model A".into(),
                "Advertised by the provider".into()
            )
            .expect("entry")
        );
        assert_eq!(
            entries[1],
            ProviderModelEntry::try_new(
                "model-b".into(),
                "Model B".into(),
                "Advertised by the provider".into()
            )
            .expect("entry")
        );
    }
}
