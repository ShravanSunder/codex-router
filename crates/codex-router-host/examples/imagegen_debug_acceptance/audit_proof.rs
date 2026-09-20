//! Redacted Router audit evidence for one native generation and edit.
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    error::Error,
    fs::OpenOptions,
    io::{Read as _, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
};

const MAX_AUDIT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditProof {
    generation_route: &'static str,
    edit_route: &'static str,
    generation_account_hash_tag: String,
    edit_account_hash_tag: String,
    selected_account_changed: bool,
}

pub fn verify_and_write(
    audit_file: &Path,
    artifact_directory: &Path,
) -> Result<AuditProof, Box<dyn Error>> {
    let metadata = std::fs::symlink_metadata(audit_file)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_AUDIT_BYTES {
        return Err("Owned Router audit is missing or exceeds the 2 MiB proof bound.".into());
    }
    let source = std::fs::File::open(audit_file)?;
    let mut text = String::new();
    source.take(MAX_AUDIT_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_AUDIT_BYTES {
        return Err("Owned Router audit exceeded the 2 MiB proof bound.".into());
    }

    let mut generation_hash = None::<String>;
    let mut edit_hash = None::<String>;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line)?;
        if event.get("outcome").and_then(Value::as_str) != Some("allowed") {
            continue;
        }
        let Some(account_hash) = event
            .get("account_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        match event.get("route_kind").and_then(Value::as_str) {
            Some("image_generations") => generation_hash = Some(account_hash.to_owned()),
            Some("image_edits") => edit_hash = Some(account_hash.to_owned()),
            _ => {}
        }
    }
    let generation_hash = generation_hash.ok_or("No allowed image_generations audit event")?;
    let edit_hash = edit_hash.ok_or("No allowed image_edits audit event")?;
    let generation_tag: String = generation_hash.chars().take(16).collect();
    let edit_tag: String = edit_hash.chars().take(16).collect();
    if generation_tag.len() < 8 || edit_tag.len() < 8 {
        return Err("Selected-account audit hash was unexpectedly short.".into());
    }
    let proof = AuditProof {
        generation_route: "image_generations",
        edit_route: "image_edits",
        generation_account_hash_tag: generation_tag,
        edit_account_hash_tag: edit_tag,
        selected_account_changed: generation_hash != edit_hash,
    };
    let redacted_events = json!([
        {"routeKind":"image_generations","outcome":"allowed","selectedAccountHashTag":proof.generation_account_hash_tag},
        {"routeKind":"image_edits","outcome":"allowed","selectedAccountHashTag":proof.edit_account_hash_tag}
    ]);
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(artifact_directory.join("selected-route-audit.json"))?;
    output.write_all(&serde_json::to_vec_pretty(&redacted_events)?)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    Ok(proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn audit_proof_records_each_redacted_selected_account_without_requiring_affinity() {
        let root = Path::new("/tmp").join(format!(
            "imagegen-audit-proof-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let audit = root.join("audit.jsonl");
        std::fs::write(
            &audit,
            concat!(
                "{\"route_kind\":\"responses\",\"outcome\":\"allowed\",\"account_hash\":\"unrelated-account-hash\"}\n",
                "{\"route_kind\":\"image_generations\",\"outcome\":\"allowed\",\"account_hash\":\"same-redacted-account-hash\"}\n",
                "{\"route_kind\":\"image_edits\",\"outcome\":\"allowed\",\"account_hash\":\"different-redacted-account-hash\"}\n"
            ),
        )
        .unwrap();
        let proof = verify_and_write(&audit, &root).unwrap();
        assert!(proof.selected_account_changed);
        assert_eq!(proof.generation_account_hash_tag, "same-redacted-ac");
        assert_eq!(proof.edit_account_hash_tag, "different-redact");
        std::fs::remove_dir_all(root).unwrap();
    }
}
