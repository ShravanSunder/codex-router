//! Fresh-thread native image generation followed by an edit of the generated conversation image.
use codex_native_integration::NativeProtocolConnection;
use serde_json::{Value, json};
use std::{
    error::Error,
    fs::OpenOptions,
    io::{Read as _, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    time::Duration,
};

const PROOF_MODEL: &str = "gpt-5.6-luna";
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

pub struct NativeProofInputs<'a> {
    pub artifact_directory: &'a Path,
    pub codex_home: &'a Path,
    pub port: u16,
}

pub struct NativeProofResult {
    pub image_capability: bool,
    pub model_image_input: bool,
    pub generated_artifact: PathBuf,
    pub edited_artifact: PathBuf,
    pub generation_bytes: u64,
    pub edit_bytes: u64,
}

struct CompletedImage {
    saved_path: PathBuf,
}

pub async fn run(
    native: &mut NativeProtocolConnection,
    inputs: NativeProofInputs<'_>,
) -> Result<NativeProofResult, Box<dyn Error>> {
    verify_effective_configuration(native, inputs.port).await?;
    let capabilities = native
        .request("modelProvider/capabilities/read", json!({}))
        .await?;
    let image_capability = capabilities.get("imageGeneration").and_then(Value::as_bool)
        == Some(true)
        && capabilities.get("namespaceTools").and_then(Value::as_bool) == Some(true);
    if !image_capability {
        return Err(
            "Debug provider did not advertise native image-generation namespace capability.".into(),
        );
    }
    let model_image_input = model_supports_image_input(native).await?;
    if !model_image_input {
        return Err("The selected proof model did not advertise image input support.".into());
    }

    let thread = create_owned_thread(native, inputs.artifact_directory).await?;
    let generation_turn = start_turn(
        native,
        &thread,
        "Use the native image generation tool exactly once. Generate a simple solid blue circle centered on a plain white background. Keep the composition minimal. Do not use shell, MCP, or any other tool.",
    )
    .await?;
    let generation = observe_completed_image(native, &thread, &generation_turn).await?;
    let generated_artifact = inputs.artifact_directory.join("generated-blue-circle.png");
    let generation_bytes = copy_verified_image(
        &generation.saved_path,
        inputs.codex_home,
        &generated_artifact,
    )?;

    let edit_turn = start_turn(
        native,
        &thread,
        "Use the native image generation tool exactly once to edit the most recent generated image. Set num_last_images_to_include to 1. Change only the blue circle to solid red while keeping the plain white background and the same minimal composition. Do not use shell, MCP, or any other tool.",
    )
    .await?;
    let edit = observe_completed_image(native, &thread, &edit_turn).await?;
    if edit.saved_path == generation.saved_path {
        return Err("Image edit reused the generation artifact path.".into());
    }
    let edited_artifact = inputs.artifact_directory.join("edited-red-circle.png");
    let edit_bytes = copy_verified_image(&edit.saved_path, inputs.codex_home, &edited_artifact)?;

    Ok(NativeProofResult {
        image_capability,
        model_image_input,
        generated_artifact,
        edited_artifact,
        generation_bytes,
        edit_bytes,
    })
}

async fn verify_effective_configuration(
    native: &mut NativeProtocolConnection,
    port: u16,
) -> Result<(), Box<dyn Error>> {
    let response = native
        .request("config/read", json!({"includeLayers":false}))
        .await?;
    let config = response
        .get("config")
        .ok_or("Missing effective configuration")?;
    let provider = config
        .pointer("/model_providers/codex-router-debug")
        .ok_or("Missing effective debug provider")?;
    if config.get("model_provider").and_then(Value::as_str) != Some("codex-router-debug")
        || provider.get("base_url").and_then(Value::as_str)
            != Some(format!("http://127.0.0.1:{port}/v1").as_str())
        || provider
            .get("requires_openai_auth")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err("Effective native provider is not the image-auth debug projection.".into());
    }
    let remote = native
        .request("remoteControl/status/read", json!({}))
        .await?;
    if remote.get("status").and_then(Value::as_str) != Some("disabled") {
        return Err("Remote Control was not disabled for the owned app-server.".into());
    }
    Ok(())
}

async fn model_supports_image_input(
    native: &mut NativeProtocolConnection,
) -> Result<bool, Box<dyn Error>> {
    let mut cursor = None::<String>;
    loop {
        let response = native
            .request(
                "model/list",
                json!({"cursor":cursor,"limit":100,"includeHidden":true}),
            )
            .await?;
        if response
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|model| {
                model.get("model").and_then(Value::as_str) == Some(PROOF_MODEL)
                    && model
                        .get("inputModalities")
                        .and_then(Value::as_array)
                        .is_some_and(|modalities| {
                            modalities
                                .iter()
                                .any(|value| value.as_str() == Some("image"))
                        })
            })
        {
            return Ok(true);
        }
        cursor = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            return Ok(false);
        }
    }
}

async fn create_owned_thread(
    native: &mut NativeProtocolConnection,
    cwd: &Path,
) -> Result<String, Box<dyn Error>> {
    let response = native
        .request(
            "thread/start",
            json!({
                "cwd":cwd,
                "model":PROOF_MODEL,
                "experimentalRawEvents":false,
                "approvalPolicy":"never",
                "approvalsReviewer":"user",
                "sandbox":"read-only",
                "config":{
                    "features.hooks":false,
                    "features.image_generation":true
                }
            }),
        )
        .await?;
    let thread = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("Native thread creation returned no identity")?
        .to_owned();
    if response.get("model").and_then(Value::as_str) != Some(PROOF_MODEL)
        || response.get("modelProvider").and_then(Value::as_str) != Some("codex-router-debug")
        || response.pointer("/sandbox/type").and_then(Value::as_str) != Some("readOnly")
        || response.get("approvalPolicy").and_then(Value::as_str) != Some("never")
    {
        return Err(
            "Fresh proof thread did not retain its model/provider/sandbox contract.".into(),
        );
    }
    Ok(thread)
}

async fn start_turn(
    native: &mut NativeProtocolConnection,
    thread: &str,
    prompt: &str,
) -> Result<String, Box<dyn Error>> {
    let response = native
        .request(
            "turn/start",
            json!({
                "threadId":thread,
                "model":PROOF_MODEL,
                "input":[{"type":"text","text":prompt,"textElements":[]}]
            }),
        )
        .await?;
    response
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Native turn start returned no identity".into())
}

async fn observe_completed_image(
    native: &mut NativeProtocolConnection,
    thread: &str,
    turn: &str,
) -> Result<CompletedImage, Box<dyn Error>> {
    tokio::time::timeout(Duration::from_secs(300), async {
        let mut saw_started = false;
        let mut completed = None;
        loop {
            let event = native.next_message().await?;
            let Some(params) = event.get("params") else {
                continue;
            };
            if params.get("threadId").and_then(Value::as_str) != Some(thread) {
                continue;
            }
            let method = event.get("method").and_then(Value::as_str);
            let event_turn = if method == Some("turn/completed") {
                params.pointer("/turn/id").and_then(Value::as_str)
            } else {
                params.get("turnId").and_then(Value::as_str)
            };
            if event_turn != Some(turn) {
                continue;
            }
            if event.get("id").is_some() {
                return Err::<CompletedImage, Box<dyn Error>>(
                    "Image proof encountered an approval callback; none was granted.".into(),
                );
            }
            match method {
                Some("item/started")
                    if params.pointer("/item/type").and_then(Value::as_str)
                        == Some("imageGeneration") =>
                {
                    saw_started = true;
                }
                Some("item/completed")
                    if params.pointer("/item/type").and_then(Value::as_str)
                        == Some("imageGeneration") =>
                {
                    let item = params.get("item").ok_or("Missing completed image item")?;
                    if item.get("status").and_then(Value::as_str) != Some("completed") {
                        return Err("Native image tool returned a terminal failure (including possible plan/quota ineligibility).".into());
                    }
                    let result_length = item
                        .get("result")
                        .and_then(Value::as_str)
                        .map(str::len)
                        .unwrap_or_default();
                    if result_length == 0 || result_length > 48 * 1024 * 1024 {
                        return Err("Native image tool returned an invalid bounded result.".into());
                    }
                    let saved_path = item
                        .get("savedPath")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(PathBuf::from)
                        .ok_or("Completed native image item omitted savedPath")?;
                    completed = Some(CompletedImage { saved_path });
                }
                Some("turn/completed")
                    if params.pointer("/turn/id").and_then(Value::as_str) == Some(turn) =>
                {
                    if params.pointer("/turn/status").and_then(Value::as_str) != Some("completed") {
                        return Err("Image proof turn did not complete successfully.".into());
                    }
                    if !saw_started {
                        return Err("Image generation tool was not exposed or invoked.".into());
                    }
                    return completed.ok_or_else(|| {
                        "Image generation tool did not emit a completed image item.".into()
                    });
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| "Timed out waiting for native image tool completion")?
}

fn copy_verified_image(
    source: &Path,
    codex_home: &Path,
    destination: &Path,
) -> Result<u64, Box<dyn Error>> {
    let canonical_source = source.canonicalize()?;
    let generated_root = codex_home.join("generated_images").canonicalize()?;
    if !canonical_source.starts_with(&generated_root) {
        return Err("Native image saved outside normal Codex generated_images.".into());
    }
    let metadata = std::fs::symlink_metadata(&canonical_source)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > MAX_IMAGE_BYTES {
        return Err("Native image artifact is not a bounded regular file.".into());
    }
    let input = std::fs::File::open(&canonical_source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(destination)?;
    let copied = std::io::copy(&mut input.take(MAX_IMAGE_BYTES + 1), &mut output)?;
    if copied != metadata.len() || copied > MAX_IMAGE_BYTES {
        return Err("Native image changed or exceeded its bound while copying.".into());
    }
    output.flush()?;
    output.sync_all()?;
    Ok(copied)
}
