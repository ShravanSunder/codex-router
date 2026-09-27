use collaboration_protocol::{ProviderSessionInspectRequest, ProviderSessionInspectResult};
use serde_json::json;

#[test]
fn provider_inspect_preserves_capability_auth_and_advertised_values() {
    let target = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"provider-session"});
    let request = json!({"target":target});
    let _: ProviderSessionInspectRequest = serde_json::from_value(request).expect("request");
    let result = json!({
        "target":target,"state":"idle","history":"historyUnavailable",
        "capabilities":{"load":true,"resume":true,"close":true,"list":true,"steer":false,
            "queue":{"kind":"router","canCancel":true},"modes":true,"configOptions":true,
            "elicitation":true,"usage":false,"promptContent":{"image":false,"audio":false,"embeddedContext":false},
            "authStatus":{"kind":"apiKey","label":"API key"}},
        "settingsCatalog":{"currentMode":"ask","modes":[{"value":"ask","label":"Ask"}],
            "configOptions":[{"id":"model","name":"Model","category":"model",
                "currentValue":{"kind":"select","value":"b"},
                "choices":[{"value":"a","label":"A"},{"value":"b","label":"B"}]}]}
    });
    let decoded: ProviderSessionInspectResult =
        serde_json::from_value(result.clone()).expect("inspect result");
    assert_eq!(serde_json::to_value(decoded).expect("round trip"), result);
}
