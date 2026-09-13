//! Shared DTO schemas. The complete Control method catalog is assembled separately.
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::Value;
use std::collections::BTreeMap;

/// Generates serialized wire shapes, retaining required nullable result fields.
/// This DTO catalog alone is not a complete Control schema or a publishable Control digest.
pub fn protocol_type_schemas() -> Result<BTreeMap<String, Value>, serde_json::Error> {
    let mut schemas = BTreeMap::new();
    add_type::<crate::EndpointDescription>(&mut schemas)?;
    add_type::<crate::ServiceManifest>(&mut schemas)?;
    add_type::<crate::SessionRef>(&mut schemas)?;
    add_type::<crate::AddressPage>(&mut schemas)?;
    add_type::<crate::JournalPage>(&mut schemas)?;
    add_type::<crate::ControlInitializationParams>(&mut schemas)?;
    add_type::<crate::ControlInitializationResult>(&mut schemas)?;
    add_type::<crate::EndpointInventory>(&mut schemas)?;
    add_type::<crate::EndpointChange>(&mut schemas)?;
    add_type::<crate::JournalStatus>(&mut schemas)?;
    add_type::<crate::JournalReadParams>(&mut schemas)?;
    add_type::<crate::AddressListParams>(&mut schemas)?;
    add_type::<crate::MessageContent>(&mut schemas)?;
    add_type::<crate::NativeSendParams>(&mut schemas)?;
    add_type::<crate::NativeSendReceipt>(&mut schemas)?;
    add_type::<crate::FiniteCommandRecord<Value, Value>>(&mut schemas)?;
    add_type::<crate::NativeObservationRecord>(&mut schemas)?;
    add_type::<crate::ConversationRecord>(&mut schemas)?;
    if let Some(schema) = schemas.get_mut("ConversationRecord") {
        crate::cli_output_contract::bind_acp_schemas(schema)?;
    }
    Ok(schemas)
}
fn add_type<TWireType: JsonSchema>(
    schemas: &mut BTreeMap<String, Value>,
) -> Result<(), serde_json::Error> {
    let schema = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<TWireType>();
    schemas.insert(
        TWireType::schema_name().into_owned(),
        serde_json::to_value(schema)?,
    );
    Ok(())
}
