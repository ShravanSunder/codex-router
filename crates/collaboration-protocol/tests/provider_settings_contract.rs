use collaboration_protocol::{
    ProviderSettingsAcceptRequest, ProviderSettingsFailure, ProviderSettingsResult,
    ProviderSettingsSetRequest, control_error_is_valid,
};
use serde_json::json;

#[test]
fn immediate_settings_contract_has_no_operation_identity() {
    let target = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"provider-session"});
    let actor = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"});
    let set = json!({"target":target,"actor":actor,"setting":"mode","value":"ask"});
    let _: ProviderSettingsSetRequest = serde_json::from_value(set).expect("set request");
    assert!(serde_json::from_value::<ProviderSettingsSetRequest>(json!({"operationId":"019c6e27-e55b-73d1-87d8-4e01f1f75101","target":target,"actor":actor,"setting":"mode","value":"ask"})).is_err());
    let _: ProviderSettingsAcceptRequest =
        serde_json::from_value(json!({"target":target,"actor":actor})).expect("accept request");
    let result = json!({"target":target,"effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},"mappingStatus":"verified","authentication":"authenticated","mode":"ask"}});
    let decoded: ProviderSettingsResult = serde_json::from_value(result.clone()).expect("result");
    assert_eq!(serde_json::to_value(decoded).expect("encode"), result);
    let failure = json!({"kind":"invalidSetting","target":target,"message":"invalid mode","setting":"mode","value":"unknown","advertised":["ask","auto"]});
    let decoded: ProviderSettingsFailure =
        serde_json::from_value(failure.clone()).expect("failure");
    assert_eq!(serde_json::to_value(decoded).expect("encode"), failure);
    let frame = json!({"jsonrpc":"2.0","id":"set","error":{"code":-32050,
        "message":"invalid mode","data":failure}});
    assert!(control_error_is_valid("conversation/settingsSet", &frame));
    assert!(!control_error_is_valid("conversation/create", &frame));
}
