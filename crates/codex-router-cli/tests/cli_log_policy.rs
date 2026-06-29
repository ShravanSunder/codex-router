use std::process::Command;

#[test]
fn rust_log_does_not_mirror_cli_logs_to_stderr_for_machine_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_codex-router"))
        .arg("--version")
        .env("RUST_LOG", "info")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .output()
        .unwrap_or_else(|error| panic!("codex-router --version should run: {error}"));

    assert!(
        output.status.success(),
        "codex-router --version should succeed: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("codex-router "),
        "version command should print version to stdout, got {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "RUST_LOG must not mirror tracing logs to machine-output stderr, got {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}
