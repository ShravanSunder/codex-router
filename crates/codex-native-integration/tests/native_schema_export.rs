use codex_native_integration::{NativeSchemaError, NativeSchemaExport, executable_identity};
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn export_uses_only_generator_arguments_and_rejects_changed_executable()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: an owned process fixture records the exact permitted command shape.
    let root = std::env::temp_dir().join(format!("schema-export-fixture-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let executable = root.join("schema-generator");
    std::fs::write(
        &executable,
        r#"#!/bin/sh
test "$#" = 5 || exit 11
test "$1" = app-server || exit 12
test "$2" = generate-json-schema || exit 13
test "$3" = --experimental || exit 14
test "$4" = --out || exit 15
printf '%s' '{"type":"object"}' > "$5/codex_app_server_protocol.schemas.json"
"#,
    )?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    let identity = executable_identity(&executable).await?;
    let output = root.join("export-output");

    // Act: export successfully, then try the stale captured identity after replacement.
    let export = NativeSchemaExport::generate(&identity, &output).await?;
    std::fs::write(&executable, "#!/bin/sh\nexit 99\n")?;
    let rejected_output = root.join("rejected-output");
    let rejected = NativeSchemaExport::generate(&identity, &rejected_output).await;

    // Assert: success is bound to identity; stale identity cannot even create an output dir.
    if export.executable() != &identity || export.bundle().canonical_bytes().is_empty() {
        return Err("export must retain its captured executable and complete bundle".into());
    }
    if std::fs::metadata(&output)?.permissions().mode() & 0o777 != 0o700 {
        return Err("export directory must be private".into());
    }
    if !matches!(rejected, Err(NativeSchemaError::ExecutableChanged)) || rejected_output.exists() {
        return Err("stale executable must fail before output directory creation".into());
    }
    std::fs::remove_file(output.join("codex_app_server_protocol.schemas.json"))?;
    std::fs::remove_dir(output)?;
    std::fs::remove_file(executable)?;
    std::fs::remove_dir(root)?;
    Ok(())
}
