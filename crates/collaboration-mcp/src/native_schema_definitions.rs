//! Codex native schema definitions that tool schemas bind their `urn:codex-native:` references to.
use codex_native_integration::{NativeSchemaBundle, NativeSchemaDigest};
use serde_json::{Map, Value};
use std::sync::Arc;

/// The Codex app-server protocol's v2 schema definitions from one native bundle, named by the
/// bundle's digest (the service manifest's `nativeSchemaDigest`).
#[derive(Clone, Debug)]
pub struct NativeSchemaDefinitions {
    digest: NativeSchemaDigest,
    definitions: Arc<Map<String, Value>>,
}

impl NativeSchemaDefinitions {
    /// The bundle's v2 definitions; `None` when the bundle carries none.
    #[must_use]
    pub fn from_bundle(bundle: &NativeSchemaBundle) -> Option<Self> {
        let document: Value = serde_json::from_slice(bundle.canonical_bytes()).ok()?;
        let definitions = document
            .pointer("/documents/codex_app_server_protocol.schemas.json/definitions/v2")?
            .as_object()?
            .clone();
        Some(Self {
            digest: NativeSchemaDigest::from(bundle),
            definitions: Arc::new(definitions),
        })
    }

    #[must_use]
    pub const fn digest(&self) -> NativeSchemaDigest {
        self.digest
    }

    pub(crate) fn definitions(&self) -> &Map<String, Value> {
        &self.definitions
    }
}
