//! One source-owned native fork; it never selects a prior user session.
use super::owned_thread_registry::OwnedThreadRegistry;
use codex_native_integration::NativeProtocolConnection;
use serde_json::json;
use std::{error::Error, path::Path};

pub async fn run_owned_fork_proof(
    native: &mut NativeProtocolConnection,
    owned: &mut OwnedThreadRegistry,
    parent: &str,
    cwd: &Path,
) -> Result<(), Box<dyn Error>> {
    owned.require_owned(parent)?;
    let receipt = owned
        .submit_text(
            native,
            parent,
            "Do not use tools or modify files. Reply exactly SELECTOR_FORK_PARENT.",
        )
        .await?;
    if owned.observe_text(native, receipt).await?.trim() != "SELECTOR_FORK_PARENT" {
        return Err("owned fork parent did not complete its marker turn".into());
    }
    let before = native
        .request(
            "thread/read",
            json!({"threadId":parent,"includeTurns":true}),
        )
        .await?;
    let child = owned.fork_owned_thread(native, parent, cwd).await?;
    let after = native
        .request(
            "thread/read",
            json!({"threadId":parent,"includeTurns":true}),
        )
        .await?;
    if before.get("thread") != after.get("thread") {
        return Err("native fork changed its owned parent's metadata or history".into());
    }
    owned.inspect(native, &child).await?;
    println!(
        "{}",
        json!({"kind":"ownedNativeForkPassed","parent":parent,"child":child,"cwd":cwd,"parentUnchanged":true,"proof":"source-side owned native API; configured picker/transport unverified"})
    );
    Ok(())
}
