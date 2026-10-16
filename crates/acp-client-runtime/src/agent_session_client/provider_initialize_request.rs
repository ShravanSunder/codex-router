//! Exact ACP v1 initialize request sent by Router's provider client.

use agent_client_protocol::schema::v1::InitializeResponse;
use agent_client_protocol::{Agent, ConnectionTo, Error, UntypedMessage};
use serde_json::json;

pub(super) async fn initialize_provider_connection(
    connection: &ConnectionTo<Agent>,
    model_picker: crate::ProviderModelPicker,
) -> Result<InitializeResponse, Error> {
    let mut capabilities = serde_json::Map::from_iter([
        ("auth".to_owned(), json!({"terminal": false})),
        ("elicitation".to_owned(), json!({"form": {}})),
    ]);
    if model_picker == crate::ProviderModelPicker::CursorParameterized {
        capabilities.insert(
            "_meta".to_owned(),
            json!({"parameterizedModelPicker": true}),
        );
    }
    let request = UntypedMessage::new(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": capabilities,
            "clientInfo": {"name": "codex-router", "version": env!("CARGO_PKG_VERSION")},
        }),
    )?;
    let response = connection.send_request(request).block_task().await?;
    serde_json::from_value(response).map_err(|_| Error::internal_error())
}
