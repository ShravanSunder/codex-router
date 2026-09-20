use std::{collections::BTreeSet, path::Path};

pub(super) fn load_advertised_native_definitions(
    service_directory: &Path,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(service_directory.join("service.json")).ok()?)
            .ok()?;
    let control_digest = manifest.get("controlSchemaDigest")?.as_str()?;
    let control_hex = validated_digest_suffix(control_digest)?;
    let control: serde_json::Value = serde_json::from_slice(
        &std::fs::read(service_directory.join(format!("control-schema-{control_hex}.json")))
            .ok()?,
    )
    .ok()?;
    let native_hex = validated_digest_suffix(control.get("x-nativeSchemaDigest")?.as_str()?)?;
    let bundle: serde_json::Value = serde_json::from_slice(
        &std::fs::read(service_directory.join(format!("{native_hex}.json"))).ok()?,
    )
    .ok()?;
    bundle
        .pointer("/documents/codex_app_server_protocol.schemas.json/definitions/v2")?
        .as_object()
        .cloned()
}

fn validated_digest_suffix(digest: &str) -> Option<&str> {
    let suffix = digest.strip_prefix("sha256:")?;
    (suffix.len() == 64 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(suffix)
}

pub(super) fn bind_native_schema_refs(
    value: &mut serde_json::Value,
    native_definitions: Option<&serde_json::Map<String, serde_json::Value>>,
) {
    let mut required = BTreeSet::new();
    collect_native_entrypoint_refs(value, &mut required);
    if let Some(definitions) = native_definitions {
        let mut pending = required.iter().cloned().collect::<Vec<_>>();
        while let Some(name) = pending.pop() {
            let Some(definition) = definitions.get(&name) else {
                continue;
            };
            let mut referenced = BTreeSet::new();
            collect_native_local_refs(definition, &mut referenced);
            for referenced_name in referenced {
                if required.insert(referenced_name.clone()) {
                    pending.push(referenced_name);
                }
            }
        }
    }
    if let (serde_json::Value::Object(root), Some(definitions)) = (&mut *value, native_definitions)
    {
        let local_definitions = root.entry("$defs").or_insert_with(|| serde_json::json!({}));
        if let Some(local_definitions) = local_definitions.as_object_mut() {
            for name in &required {
                let Some(definition) = definitions.get(name) else {
                    continue;
                };
                let mut definition = definition.clone();
                rebase_native_definition_refs(&mut definition);
                local_definitions.insert(format!("codexNative{name}"), definition);
            }
        }
    }
    replace_native_entrypoint_refs(value, native_definitions.is_some());
}

fn collect_native_entrypoint_refs(value: &serde_json::Value, names: &mut BTreeSet<String>) {
    if let Some(name) = value
        .get("$ref")
        .and_then(serde_json::Value::as_str)
        .and_then(|reference| reference.strip_prefix("urn:codex-native:"))
    {
        names.insert(name.to_owned());
    }
    collect_child_refs(value, names, collect_native_entrypoint_refs);
}

fn collect_native_local_refs(value: &serde_json::Value, names: &mut BTreeSet<String>) {
    if let Some(name) = value
        .get("$ref")
        .and_then(serde_json::Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/definitions/v2/"))
    {
        names.insert(name.to_owned());
    }
    collect_child_refs(value, names, collect_native_local_refs);
}

fn collect_child_refs(
    value: &serde_json::Value,
    names: &mut BTreeSet<String>,
    collect: fn(&serde_json::Value, &mut BTreeSet<String>),
) {
    match value {
        serde_json::Value::Object(fields) => {
            for child in fields.values() {
                collect(child, names);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect(child, names);
            }
        }
        _ => {}
    }
}

fn rebase_native_definition_refs(value: &mut serde_json::Value) {
    if let Some(reference) = value.get_mut("$ref")
        && let Some(name) = reference
            .as_str()
            .and_then(|reference| reference.strip_prefix("#/definitions/v2/"))
    {
        *reference = serde_json::json!(format!("#/$defs/codexNative{name}"));
    }
    match value {
        serde_json::Value::Object(fields) => {
            for child in fields.values_mut() {
                rebase_native_definition_refs(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                rebase_native_definition_refs(child);
            }
        }
        _ => {}
    }
}

fn replace_native_entrypoint_refs(value: &mut serde_json::Value, available: bool) {
    if let Some(name) = value
        .get("$ref")
        .and_then(serde_json::Value::as_str)
        .and_then(|reference| reference.strip_prefix("urn:codex-native:"))
    {
        *value = if available {
            serde_json::json!({"$ref":format!("#/$defs/codexNative{name}")})
        } else {
            serde_json::json!({})
        };
        return;
    }
    match value {
        serde_json::Value::Object(fields) => {
            for child in fields.values_mut() {
                replace_native_entrypoint_refs(child, available);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                replace_native_entrypoint_refs(child, available);
            }
        }
        _ => {}
    }
}
