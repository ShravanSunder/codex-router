//! Reuse compiled validators only for the identical content-addressed native schema bundle.
use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct NativeSchemaCache {
    cached: Option<([u8; 32], Option<Arc<NativePayloadSchemas>>)>,
}
impl NativeSchemaCache {
    pub fn resolve(&mut self, bundle: &NativeSchemaBundle) -> Option<Arc<NativePayloadSchemas>> {
        if let Some((digest, schemas)) = &self.cached
            && digest == bundle.digest()
        {
            return schemas.clone();
        }
        let schemas = NativePayloadSchemas::from_bundle(bundle).ok().map(Arc::new);
        self.cached = Some((*bundle.digest(), schemas.clone()));
        schemas
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use codex_native_integration::NativeOperation;
    use serde_json::json;
    use std::collections::BTreeMap;
    fn bundle(queue: bool) -> Result<NativeSchemaBundle, Box<dyn std::error::Error>> {
        let mut defs = serde_json::Map::new();
        for name in [
            "ThreadRead",
            "ThreadResume",
            "ThreadStart",
            "ThreadLoadedList",
            "TurnStart",
            "TurnSteer",
            "TurnInterrupt",
        ] {
            defs.insert(format!("{name}Params"), json!({"type":"object"}));
            defs.insert(format!("{name}Response"), json!({"type":"object"}));
        }
        if queue {
            defs.insert("ThreadQueueAddParams".into(), json!({"type":"object"}));
            defs.insert("ThreadQueueAddResponse".into(), json!({"type":"object"}));
        }
        Ok(NativeSchemaBundle::from_documents(BTreeMap::from([(
            "codex_app_server_protocol.schemas.json".into(),
            serde_json::to_vec(&json!({"definitions":{"v2":defs}}))?,
        )]))?)
    }
    #[test]
    fn same_bundle_reuses_validation_and_changed_bundle_changes_capability() {
        // Arrange: two valid profiles with observably different queue support.
        let original = bundle(false).unwrap();
        let changed = bundle(true).unwrap();
        let mut cache = NativeSchemaCache::default();
        // Act.
        let first = cache.resolve(&original).unwrap();
        let repeated = cache.resolve(&original).unwrap();
        let next = cache.resolve(&changed).unwrap();
        // Assert: a restart avoids compilation, but new content never borrows old admission.
        assert!(Arc::ptr_eq(&first, &repeated));
        assert!(!first.supports_operation(NativeOperation::QueueAdd));
        assert!(next.supports_operation(NativeOperation::QueueAdd));
        assert!(!Arc::ptr_eq(&first, &next));
    }
}
