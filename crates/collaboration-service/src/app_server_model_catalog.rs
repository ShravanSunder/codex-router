//! Codex app-server model-list projection over the supervisor-owned live catalog.
use serde_json::{Value, json};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderModelEntry {
    id: String,
    display_name: String,
    description: String,
}

#[derive(Debug, thiserror::Error)]
#[error("provider model entry requires a nonempty id and display name")]
pub struct InvalidProviderModelEntry;

impl ProviderModelEntry {
    pub fn try_new(
        id: String,
        display_name: String,
        description: String,
    ) -> Result<Self, InvalidProviderModelEntry> {
        if id.is_empty()
            || display_name.is_empty()
            || id.contains('\0')
            || display_name.contains('\0')
        {
            return Err(InvalidProviderModelEntry);
        }
        Ok(Self {
            id,
            display_name,
            description,
        })
    }

    fn provider_default() -> Self {
        Self {
            id: "provider-default".into(),
            display_name: "provider default".into(),
            description: "The provider selects its default model".into(),
        }
    }

    fn as_app_server_model(&self, is_default: bool) -> Value {
        json!({
            "id":self.id,"model":self.id,
            "upgrade":null,"upgradeInfo":null,"availabilityNux":null,
            "displayName":self.display_name,"description":self.description,
            "hidden":false,"supportedReasoningEfforts":[],
            "defaultReasoningEffort":"medium","multiAgentVersion":null,
            "isDefault":is_default
        })
    }
}

pub(crate) fn render_model_list(catalog: &[ProviderModelEntry]) -> Value {
    let models = if catalog.is_empty() {
        vec![ProviderModelEntry::provider_default()]
    } else {
        catalog.to_vec()
    };
    json!({
        "data":models.iter().enumerate()
            .map(|(index, model)| model.as_app_server_model(index == 0))
            .collect::<Vec<_>>(),
        "nextCursor":null
    })
}
