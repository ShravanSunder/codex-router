//! Internal schema templates bound to an exact native bundle by the Control schema builder.
use schemars::{Schema, SchemaGenerator, json_schema};
pub(crate) fn thread_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"$ref":"urn:codex-native:Thread"})
}
pub(crate) fn status_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"$ref":"urn:codex-native:ThreadStatus"})
}
