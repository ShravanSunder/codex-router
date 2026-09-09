use codex_native_integration::{NativeOperation, NativePayloadSchemas, NativeSchemaBundle};
use serde_json::json;
use std::collections::BTreeMap;
#[test]
fn fork_requires_its_own_exported_schema() -> Result<(), Box<dyn std::error::Error>> {
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":&definitions}}))?,
    )]))?;
    let base = NativePayloadSchemas::from_bundle(&bundle)?;
    if base.supports_operation(NativeOperation::ForkThread) {
        return Err("fork admitted without its exported schema".into());
    }
    definitions.insert("ThreadForkParams".into(),json!({"type":"object","properties":{"threadId":{"type":"string"},"lastTurnId":{"type":"string"}},"required":["threadId","lastTurnId"],"additionalProperties":false}));
    definitions.insert("ThreadForkResponse".into(), json!({"type":"object"}));
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    if !NativePayloadSchemas::from_bundle(&bundle)?.supports_operation(NativeOperation::ForkThread)
    {
        return Err("exported fork operation was not admitted".into());
    }
    Ok(())
}
