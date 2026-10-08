//! Internal schema templates whose native references bind to an exact native bundle.
use schemars::{Schema, SchemaGenerator, json_schema};
pub(crate) fn thread_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"$ref":"urn:codex-native:Thread"})
}
pub(crate) fn status_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"$ref":"urn:codex-native:ThreadStatus"})
}
