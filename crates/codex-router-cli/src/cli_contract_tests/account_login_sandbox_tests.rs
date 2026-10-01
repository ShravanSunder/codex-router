use super::account_login_tests::openai_device_login_activates_fake_issuer_tokens_without_plaintext_files;
use super::cli_test_support::TestRoot;
use super::cli_test_support::must_ok;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const SANDBOX_LOGIN_CHILD_ENV: &str = "CODEX_ROUTER_OPENAI_LOGIN_SANDBOX_CHILD";
const SANDBOX_PROBE_PATH_ENV: &str = "CODEX_ROUTER_OPENAI_LOGIN_SANDBOX_PROBE_PATH";

#[test]
fn openai_device_login_succeeds_when_native_auth_reads_are_denied() {
    if let Some(probe_path) = std::env::var_os(SANDBOX_PROBE_PATH_ENV) {
        let read_error = fs::read(probe_path).expect_err("sandbox should deny the probe read");
        assert_eq!(read_error.kind(), std::io::ErrorKind::PermissionDenied);
        return;
    }

    if std::env::var_os(SANDBOX_LOGIN_CHILD_ENV).is_some() {
        openai_device_login_activates_fake_issuer_tokens_without_plaintext_files();
        return;
    }

    let test_root = TestRoot::new("openai-login-sandbox-probe");
    must_ok(fs::create_dir_all(test_root.path()));
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME should be available"));
    let blocked_native_auth_paths = [
        home.join(".codex").join("auth.json"),
        home.join(".claude").join(".credentials.json"),
    ];
    let probe_file_path = test_root.path().join("probe-auth-file");
    must_ok(fs::write(&probe_file_path, b"synthetic sandbox probe"));
    let denied_probe_path = must_ok(fs::canonicalize(probe_file_path));
    let mut canary_denied_paths = blocked_native_auth_paths.to_vec();
    canary_denied_paths.push(denied_probe_path.clone());
    let test_binary = std::env::current_exe().expect("test binary path should be available");
    let probe_output = Command::new("/usr/bin/sandbox-exec")
        .arg("-p")
        .arg(sandbox_profile_denying_file_reads(&canary_denied_paths))
        .arg(&test_binary)
        .arg("--exact")
        .arg("tests::account_login_sandbox_tests::openai_device_login_succeeds_when_native_auth_reads_are_denied")
        .arg("--nocapture")
        .env(SANDBOX_PROBE_PATH_ENV, &denied_probe_path)
        .output()
        .expect("sandbox-exec should start the file-read probe");
    assert!(
        probe_output.status.success(),
        "sandbox should deny the synthetic file read; stderr: {}",
        String::from_utf8_lossy(&probe_output.stderr)
    );

    let sandbox_profile = sandbox_profile_denying_file_reads(&blocked_native_auth_paths);
    let login_start_unix_seconds = unix_timestamp_seconds();
    let login_output = Command::new("/usr/bin/sandbox-exec")
        .arg("-p")
        .arg(sandbox_profile)
        .arg(test_binary)
        .arg("--exact")
        .arg("tests::account_login_sandbox_tests::openai_device_login_succeeds_when_native_auth_reads_are_denied")
        .arg("--nocapture")
        .env(SANDBOX_LOGIN_CHILD_ENV, "1")
        .output()
        .expect("sandbox-exec should start the OpenAI login test");
    assert!(
        login_output.status.success(),
        "sandboxed OpenAI device login should succeed; stderr: {}",
        String::from_utf8_lossy(&login_output.stderr)
    );

    for blocked_path in &blocked_native_auth_paths {
        let blocked_credential_read_records =
            sandbox_violation_records(blocked_path, login_start_unix_seconds);
        assert!(
            blocked_credential_read_records.is_empty(),
            "supplementary sandbox violation log should be empty for a protected native auth path"
        );
    }
}

fn sandbox_violation_records(path: &Path, start_unix_seconds: u64) -> Vec<serde_json::Value> {
    let escaped_path = escape_predicate_string(path);
    let predicate = format!(
        "subsystem == \"com.apple.sandbox.reporting\" AND category == \"violation\" AND (eventMessage CONTAINS[c] \"{escaped_path}\" OR composedMessage CONTAINS[c] \"{escaped_path}\")"
    );
    let log_output = Command::new("/usr/bin/log")
        .arg("show")
        .arg("--start")
        .arg(format!("@{start_unix_seconds}"))
        .arg("--style")
        .arg("json")
        .arg("--info")
        .arg("--debug")
        .arg("--predicate")
        .arg(predicate)
        .output()
        .expect("unified logging should be available for sandbox proof");
    assert!(
        log_output.status.success(),
        "sandbox violation log query should succeed; stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );
    serde_json::from_slice(&log_output.stdout).expect("sandbox violation log should be JSON")
}

fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sandbox_profile_denying_file_reads(paths: &[PathBuf]) -> String {
    let denied_paths = paths
        .iter()
        .map(|path| {
            format!(
                "(deny file-read* (literal \"{}\"))",
                escape_predicate_string(path)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("(version 1)\n(allow default)\n{denied_paths}\n")
}

fn escape_predicate_string(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}
