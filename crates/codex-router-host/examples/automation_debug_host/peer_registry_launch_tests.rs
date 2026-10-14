//! Private registry admission and bounded task-context retention for the debug example.
use super::*;

fn registry_options(root: &Path) -> launch_options::DebugHostOptions {
    parse_options_from([
        OsString::from("--run-directory"),
        root.as_os_str().to_owned(),
        OsString::from("--router-binary"),
        OsString::from("/tmp/router"),
        OsString::from("--peer-registry-directory"),
        root.join("peer-registry").into_os_string(),
    ])
    .expect("task-owned registry input")
    .expect("launch options")
}

#[test]
fn peer_registry_parser_rejects_paths_outside_the_selected_run() {
    for directory in [
        "relative/peer-registry",
        "/tmp/other/peer-registry",
        "/tmp/proof/../peer-registry",
    ] {
        let parsed = parse_options_from(
            [
                "--run-directory",
                "/tmp/proof",
                "--router-binary",
                "/tmp/router",
                "--peer-registry-directory",
                directory,
            ]
            .map(OsString::from),
        );
        assert!(parsed.is_err(), "foreign registry accepted: {directory}");
    }
}

#[test]
fn peer_registry_selection_is_preserved_in_replacement_argv() {
    let options = registry_options(Path::new("/tmp/proof"));
    let expected = [
        "--resume-run-directory",
        "/tmp/proof",
        "--router-binary",
        "/tmp/router",
        "--port",
        "18787",
        "--peer-registry-directory",
        "/tmp/proof/peer-registry",
    ]
    .map(OsString::from)
    .to_vec();
    assert_eq!(options.resume_arguments(), expected);
    let resumed = parse_options_from(expected)
        .expect("replacement parse")
        .expect("options");
    assert_eq!(
        resumed.peer_registry_directory,
        Some("/tmp/proof/peer-registry".into())
    );
}

#[test]
fn peer_registry_resume_rejects_added_removed_or_changed_selection() {
    let run = TestRunDirectory::create().expect("private run");
    run.write_context(stopped_process_id().expect("stopped process"))
        .expect("context");
    let options = registry_options(&run.0);
    let mut context = read_existing_context(&run.0).expect("context read");
    assert!(validate_resume_binding(&context, &options).is_err());
    context.peer_registry_directory = options.peer_registry_directory.clone();
    replace_resumed_context(&context).expect("retain registry");
    let context = read_existing_context(&run.0).expect("round trip");
    validate_resume_binding(&context, &options).expect("same registry");
    let mut removed = options.clone();
    removed.peer_registry_directory = None;
    assert!(validate_resume_binding(&context, &removed).is_err());
    let mut changed = options;
    changed.peer_registry_directory = Some("/tmp/foreign/peer-registry".into());
    assert!(validate_resume_binding(&context, &changed).is_err());
}

#[test]
fn oversized_context_is_refused_without_replacing_the_prior_marker() {
    let run = TestRunDirectory::create().expect("private run");
    run.write_context(stopped_process_id().expect("stopped process"))
        .expect("context");
    let marker = run.0.join("debug-host-context.json");
    let original = std::fs::read(&marker).expect("original bytes");
    let mut context = read_existing_context(&run.0).expect("context");
    context.cursor_provider = Some(ProviderCommand {
        executable: "/usr/bin/python3".into(),
        arguments: vec!["x".repeat(4096)],
    });
    assert!(replace_resumed_context(&context).is_err());
    assert_eq!(std::fs::read(&marker).expect("unchanged marker"), original);
    assert!(std::fs::read_dir(&run.0).expect("directory").all(|entry| {
        !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
}

#[test]
fn registry_preparation_creates_only_the_fresh_owned_child_and_resume_preserves_it() {
    let run = TestRunDirectory::create().expect("private run");
    let mut options = registry_options(&run.0);
    let registry = run.0.join("peer-registry");
    assert!(!registry.exists());
    prepare_peer_registry(&options).expect("fresh registry after run admission");
    assert_eq!(
        std::fs::metadata(&registry)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    std::fs::write(registry.join("owned-marker"), b"same-registry").expect("marker");
    assert!(
        prepare_peer_registry(&options).is_err(),
        "fresh mode must not reuse a registry"
    );
    options.run_directory_admission = RunDirectoryAdmission::Resume;
    prepare_peer_registry(&options).expect("resume selected owned registry");
    assert_eq!(
        std::fs::read(registry.join("owned-marker")).expect("marker"),
        b"same-registry"
    );
    std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o755))
        .expect("public fixture");
    assert!(prepare_peer_registry(&options).is_err());
    assert!(!run.0.join("home").exists());
}

#[test]
fn registry_preparation_rejects_a_symlink_even_with_an_owned_target() {
    let run = TestRunDirectory::create().expect("private run");
    let other = TestRunDirectory::create().expect("other private run");
    let mut options = registry_options(&run.0);
    options.run_directory_admission = RunDirectoryAdmission::Resume;
    std::os::unix::fs::symlink(&other.0, run.0.join("peer-registry")).expect("symlink fixture");
    assert!(prepare_peer_registry(&options).is_err());
}

#[test]
fn scripted_cursor_context_round_trip_fits_the_existing_read_limit() {
    let run = TestRunDirectory::create().expect("private run");
    run.write_context(stopped_process_id().expect("stopped process"))
        .expect("context");
    let record_path = run.0.join("provider-prompt-receipts.jsonl");
    let steps = serde_json::json!([
        {"action":"expect_request","requestName":"initialize","method":"initialize","params":{"protocolVersion":1}},
        {"action":"respond","requestName":"initialize","result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":"matrix-recipient","version":"1"}}},
        {"action":"expect_request","requestName":"create-target","method":"session/new","params":{}},
        {"action":"respond","requestName":"create-target","result":{"sessionId":"matrix-acp-target"}},
        {"action":"expect_request","requestName":"target-prompt-0","method":"session/prompt","params":{"sessionId":"matrix-acp-target"},"recordPath":record_path},
        {"action":"respond","requestName":"target-prompt-0","result":{"stopReason":"end_turn"}}
    ]);
    let command = ProviderCommand {
        executable: "/usr/bin/python3".into(),
        arguments: vec![
            "-u".into(),
            format!(
                "{}/src/external_provider_runtime/acp_scripted_fixture.py",
                env!("CARGO_MANIFEST_DIR")
            ),
            serde_json::to_string(&steps).expect("script steps"),
        ],
    };
    let mut context = read_existing_context(&run.0).expect("context");
    context.cursor_provider = Some(command.clone());
    context.router_root = Some("/tmp/sqlite-proof-account-20261011-s1".into());
    assert!(
        serialize_context(&context)
            .expect("bounded serialization")
            .len()
            <= 4096
    );
    replace_resumed_context(&context).expect("write bounded context");
    assert_eq!(
        read_existing_context(&run.0)
            .expect("existing reader")
            .cursor_provider,
        Some(command)
    );
    assert!(
        !record_path.exists(),
        "context proof must not execute a provider"
    );
}
