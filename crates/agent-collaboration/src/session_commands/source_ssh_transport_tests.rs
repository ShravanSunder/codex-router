use super::super::router_connection_registry::RouterConnectionRegistry;
use super::*;
use serde_json::json;

fn ssh_profile() -> RouterConnectionProfile {
    RouterConnectionRegistry::parse(
        &json!({"version":1,"routers":[{
            "name":"Source", "connection":{"kind":"ssh",
                "serviceId":"00000000-0000-4000-8000-000000000001",
                "destination":"source.example.invalid", "executable":"/bin/echo",
                "serviceDirectory":"/private/source's root", "clientProfile":"routerDebug"},
            "defaultRemoteCwd":"/source-only/project"
        }]})
        .to_string(),
    )
    .unwrap()
    .routers
    .remove(0)
}

#[test]
fn source_ssh_native_options_preserve_only_explicit_model_effort_and_display() {
    let arguments = [
        "--model=chosen-model",
        "-c",
        "model_reasoning_effort='high'",
        "--no-alt-screen",
    ]
    .map(OsString::from);
    let options = SourceNativeOptions::parse(&arguments).unwrap();
    assert_eq!(options.model.as_deref(), Some("chosen-model"));
    assert_eq!(options.reasoning_effort.as_deref(), Some("high"));
    assert!(options.no_alt_screen);
    for arguments in [
        vec!["--remote", "unix:///wrong.sock"],
        vec!["--cd=/caller/path"],
        vec!["--profile", "other"],
        vec!["--remote-auth-token-env", "SECRET_CANARY"],
        vec!["-c", "model_provider='other'"],
        vec!["--sandbox", "danger-full-access"],
        vec!["--ask-for-approval=never"],
        vec!["--yolo"],
        vec!["initial prompt"],
        vec!["--model=one", "-m", "two"],
        vec!["--no-alt-screen", "--no-alt-screen"],
        vec!["-c", "model='one'\nmodel_reasoning_effort='high'"],
        vec!["--model"],
    ] {
        let arguments = arguments
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert_eq!(
            SourceNativeOptions::parse(&arguments),
            Err(SourceRejection::ConflictingArguments)
        );
    }
}

#[test]
fn source_ssh_quotes_preserve_literal_shell_metacharacters_without_execution() {
    let literal = "one ' $(printf EXPANDED) ; `printf EXPANDED` \\\" two";
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("printf '%s' {}", shell_quote(literal))])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, literal.as_bytes());
}

#[test]
fn source_ssh_command_uses_fixed_entrypoint_strict_trust_and_one_json_argument() {
    let profile = ssh_profile();
    let request = request_for_profile(&profile, SourceOperation::Endpoints).unwrap();
    let RouterConnectionTransport::Ssh(connection) = &profile.transport else {
        panic!("SSH fixture")
    };
    let arguments = ssh_arguments(connection, &request, false).unwrap();
    assert_eq!(
        &arguments[..9],
        [
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=5",
            "--",
            "source.example.invalid"
        ]
        .map(OsString::from)
    );
    let command = arguments.last().unwrap().to_str().unwrap();
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", command])
        .output()
        .unwrap();
    assert!(output.status.success());
    let expected = format!(
        "--source-request-v1 {}\n",
        serde_json::to_string(&request).unwrap()
    );
    assert_eq!(output.stdout, expected.as_bytes());
}

#[test]
fn source_ssh_protocol_rejects_unknown_fields_and_unvalidated_paths() {
    let request = json!({"serviceDirectory":"/private/source", "expectedService":"00000000-0000-4000-8000-000000000001", "operation":{"kind":"prepare", "action":{"kind":"new", "directory":"/source-only"},"clientProfile":"routerDebug"}});
    assert!(serde_json::from_value::<SourceRequest>(request.clone()).is_ok());
    for bad_path in ["relative", "/source\ncommand"] {
        let mut changed = request.clone();
        changed["serviceDirectory"] = json!(bad_path);
        assert!(serde_json::from_value::<SourceRequest>(changed).is_err());
    }
    let mut changed = request;
    changed["operation"]["approvalPolicy"] = json!("never");
    assert!(serde_json::from_value::<SourceRequest>(changed).is_err());
    assert!(
        serde_json::from_value::<SourceNativeOptions>(
            json!({"model":null,"reasoningEffort":null,"noAltScreen":false,"remote":"wrong"})
        )
        .is_err()
    );
}

#[test]
fn source_ssh_cross_service_action_is_rejected_before_transport() {
    let profile = ssh_profile();
    let target = serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000009","endpointId":"codex-local"},"sessionId":"00000000-0000-4000-8000-000000000002"})).unwrap();
    let action = SourceSessionAction::Fork {
        target,
        stored_model: Default::default(),
    };
    assert_eq!(
        action.validate(&profile.service_id),
        Err(SourceRejection::WrongService)
    );
}
