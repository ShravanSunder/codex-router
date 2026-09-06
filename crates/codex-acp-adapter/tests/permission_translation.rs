use codex_acp_adapter::{AcpSchemaCatalog, PendingPermission};
use serde_json::json;

#[test]
fn native_choices_preserve_order_and_invalid_selection_cancels_without_grant() {
    let mut schema = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("schema: {error}"));
    let generation: communication_protocol::CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap_or_else(|error| panic!("generation: {error}"));
    let native = json!({"id":9223372036854775807_i64,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread","turnId":"turn","itemId":"tool","command":"cargo test","reason":"Validate the change","availableDecisions":["decline",{"acceptWithExecpolicyAmendment":{}},"accept"]}});
    for (option, expected, invalid) in [
        ("native-accept", "accept", false),
        ("native-accept-session", "cancel", true),
    ] {
        let permission = PendingPermission::translate(
            &mut schema,
            generation.clone(),
            "permission-1".into(),
            &native,
        )
        .unwrap_or_else(|error| panic!("translate: {error}"));
        assert_eq!(
            permission.request()["params"]["options"][0]["optionId"],
            "native-decline"
        );
        assert!(
            permission.request()["params"]["toolCall"]["content"][0]["content"]["text"]
                .as_str()
                .is_some_and(
                    |text| text.contains("cargo test") && text.contains("Validate the change")
                )
        );
        assert_eq!(
            permission.request()["params"]["options"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        let reply = permission.resolve(&mut schema,&generation,&json!({"jsonrpc":"2.0","id":"permission-1","result":{"outcome":{"outcome":"selected","optionId":option}}})).unwrap_or_else(|error| panic!("resolve: {error}"));
        assert_eq!(reply.native_response["id"], native["id"]);
        assert_eq!(reply.native_response["result"]["decision"], expected);
        assert_eq!(reply.invalid_selection, invalid);
        assert!(reply.native_response.get("jsonrpc").is_none());
    }
}

#[test]
fn cancelled_permission_is_native_cancel_and_old_generation_cannot_respond() {
    let mut schema = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("schema: {error}"));
    let generation: communication_protocol::CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap_or_else(|error| panic!("generation: {error}"));
    let native = json!({"id":"native-callback","method":"item/fileChange/requestApproval","params":{"threadId":"thread","turnId":"turn","itemId":"file-change"}});
    let reply = PendingPermission::translate(
        &mut schema,
        generation.clone(),
        "permission-1".into(),
        &native,
    )
    .unwrap_or_else(|error| panic!("translate: {error}"))
    .resolve(
        &mut schema,
        &generation,
        &json!({"jsonrpc":"2.0","id":"permission-1","result":{"outcome":{"outcome":"cancelled"}}}),
    )
    .unwrap_or_else(|error| panic!("resolve: {error}"));
    assert_eq!(reply.native_response["result"]["decision"], "cancel");
    assert!(!reply.invalid_selection);
    let pending = PendingPermission::translate(
        &mut schema,
        generation.clone(),
        "permission-2".into(),
        &native,
    )
    .unwrap_or_else(|error| panic!("translate: {error}"));
    let mut next = generation;
    next.generation = 2_u64
        .try_into()
        .unwrap_or_else(|error| panic!("generation: {error}"));
    assert!(pending.resolve(&mut schema,&next,&json!({"jsonrpc":"2.0","id":"permission-2","result":{"outcome":{"outcome":"selected","optionId":"native-accept"}}})).is_err());
}
