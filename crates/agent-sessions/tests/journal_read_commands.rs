#[test]
fn journal_commands_report_missing_service_as_structured_unavailable() {
    for command in [
        vec!["journal", "status"],
        vec![
            "journal",
            "read",
            "--endpoint",
            "codex-local",
            "--journal-id",
            "00000000-0000-4000-8000-000000000001",
            "--after",
            "0",
        ],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .args(command)
            .args([
                "--json",
                "--service-directory",
                "/tmp/missing-journal-service-proof",
            ])
            .output()
            .unwrap_or_else(|error| panic!("command: {error}"));
        assert_eq!(output.status.code(), Some(3));
        let result: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("output: {error}"));
        assert_eq!(result["error"]["kind"], "unavailable");
    }
}
