//! Validate a previously exported schema without starting a Codex runtime.
use codex_native_integration::NativeSchemaBundle;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("expected schema export directory")?;
    let generated;
    let collected;
    let bundle = if let Some(executable) = std::env::args_os().nth(2) {
        let identity =
            codex_native_integration::executable_identity(std::path::Path::new(&executable))
                .await?;
        generated = codex_native_integration::NativeSchemaExport::generate(
            &identity,
            std::path::Path::new(&path),
        )
        .await?;
        generated.bundle()
    } else {
        collected = NativeSchemaBundle::from_export_directory(std::path::Path::new(&path))?;
        &collected
    };
    println!("canonical bytes: {}", bundle.canonical_bytes().len());
    print!("sha256:");
    for byte in bundle.digest() {
        print!("{byte:02x}");
    }
    println!();
    for definition in [
        "UserInput",
        "Thread",
        "ThreadStatus",
        "TurnStartResponse",
        "TurnSteerResponse",
        "ThreadResumeResponse",
    ] {
        let validator = bundle.validator_for_v2(definition)?;
        if definition == "UserInput"
            && (!validator
                .is_valid(&serde_json::json!({"type":"text","text":"schema validation only"}))
                || validator.is_valid(&serde_json::json!({"type":"text","text":42})))
        {
            return Err("native input schema validation failed".into());
        }
        println!("compiled native definition: {definition}");
    }
    let operations = codex_native_integration::NativePayloadSchemas::from_bundle(bundle)?;
    println!("compiled parameter/result schemas for all seven native helper operations");
    if !operations.supports_server_messages() {
        return Err("native server envelope schemas unavailable".into());
    }
    if operations
        .validates_server_message(&serde_json::json!({"method":"turn/completed","params":{}}))
    {
        return Err("malformed native event passed schema validation".into());
    }
    println!("compiled server request/notification envelopes; rejected malformed completion");
    Ok(())
}
