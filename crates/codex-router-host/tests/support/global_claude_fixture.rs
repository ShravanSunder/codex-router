//! Real common Host startup fixture with a scripted external ACP bridge only.

use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_client::CollaborationClient;
use collaboration_protocol::{ChannelDescription, EndpointAvailability};
use serde::Deserialize;
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

pub const GLOBAL_VERSION: &str = "9.9.1-global-fixture";
pub const BUNDLED_VERSION: &str = "0.0.1-bundled-fixture";

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum HarnessIdentity {
    GlobalHarness,
    BundledHarness,
}

#[derive(Debug, Deserialize)]
pub struct NativeExecutionReceipt {
    pub invoked_executable: PathBuf,
    pub identity: HarnessIdentity,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct AdapterDispatchReceipt {
    pub selected_executable: PathBuf,
    pub version_stdout: String,
}

pub struct HarnessSelectionFixture {
    _private_root: tempfile::TempDir,
    runtime_directory: PathBuf,
    pub global_directory: PathBuf,
    pub global_executable: PathBuf,
    pub bundled_executable: PathBuf,
    adapter_executable: PathBuf,
    adapter_dispatch_receipt: PathBuf,
    native_execution_receipt: PathBuf,
}

pub struct StartupObservation {
    pub availability: EndpointAvailability,
    pub adapter_dispatch: Option<AdapterDispatchReceipt>,
    pub native_execution: Option<NativeExecutionReceipt>,
    pub published_channels: Vec<ChannelDescription>,
}

impl HarnessSelectionFixture {
    pub fn create(include_global_executable: bool) -> Self {
        let private_root = tempfile::Builder::new()
            .prefix("global-harness-")
            .tempdir_in("/tmp")
            .expect("private fixture root with bounded Unix socket paths");
        fs::set_permissions(private_root.path(), fs::Permissions::from_mode(0o700))
            .expect("private fixture root permissions");
        let runtime_directory = private_root.path().join("runtime");
        let global_directory = private_root.path().join("global-bin");
        let bundled_directory = private_root.path().join("sdk-bundled");
        for directory in [&runtime_directory, &global_directory, &bundled_directory] {
            fs::create_dir(directory).expect("fixture directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("private fixture directory permissions");
        }
        let global_executable = global_directory.join("claude");
        let bundled_executable = bundled_directory.join("claude");
        if include_global_executable {
            Self::write_native_executable(&global_executable, "globalHarness", GLOBAL_VERSION);
        }
        Self::write_native_executable(&bundled_executable, "bundledHarness", BUNDLED_VERSION);
        let adapter_executable = private_root.path().join("claude-agent-acp.py");
        fs::write(
            &adapter_executable,
            r#"#!/usr/bin/python3
import json, os, pathlib, subprocess, sys

request = json.loads(sys.stdin.readline())
assert request['method'] == 'initialize'
selected_executable = os.environ.get('CLAUDE_CODE_EXECUTABLE') or sys.argv[1]
dispatch_path = pathlib.Path(os.environ['GLOBAL_HARNESS_FIXTURE_ADAPTER_RECEIPT'])
dispatch_path.write_text(json.dumps({
    'selected_executable': str(pathlib.Path(selected_executable).resolve()),
    'version_stdout': '',
}))
native_output = subprocess.run(
    [selected_executable, '--version'],
    check=True, capture_output=True, text=True,
)
dispatch_path.write_text(json.dumps({
    'selected_executable': str(pathlib.Path(selected_executable).resolve()),
    'version_stdout': native_output.stdout.strip(),
}))
print(json.dumps({
    'jsonrpc': '2.0', 'id': request['id'], 'result': {
        'protocolVersion': 1,
        'agentCapabilities': {},
        'agentInfo': {'name': 'global-harness-selection-fixture', 'version': '1'},
    },
}), flush=True)
sys.stdin.read()
"#,
        )
        .expect("scripted external ACP adapter");
        fs::set_permissions(&adapter_executable, fs::Permissions::from_mode(0o700))
            .expect("executable ACP adapter fixture");
        let adapter_dispatch_receipt = private_root.path().join("adapter-dispatch.json");
        let native_execution_receipt = private_root.path().join("native-execution.json");
        Self {
            _private_root: private_root,
            runtime_directory,
            global_directory,
            global_executable,
            bundled_executable,
            adapter_executable,
            adapter_dispatch_receipt,
            native_execution_receipt,
        }
    }

    fn write_native_executable(executable: &Path, identity: &str, version: &str) {
        let script = format!(
            r#"#!/usr/bin/python3
import json, os, pathlib, sys
assert sys.argv[1:] == ['--version']
receipt = {{
    'invoked_executable': str(pathlib.Path(sys.argv[0]).resolve()),
    'identity': '{identity}',
    'version': '{version}',
}}
pathlib.Path(os.environ['GLOBAL_HARNESS_FIXTURE_NATIVE_RECEIPT']).write_text(json.dumps(receipt))
print('{version}', flush=True)
"#,
        );
        fs::write(executable, script).expect("native fixture executable");
        fs::set_permissions(executable, fs::Permissions::from_mode(0o700))
            .expect("native fixture executable permissions");
    }

    pub async fn observe_startup(&self) -> StartupObservation {
        self.observe_startup_with_environment(Vec::new()).await
    }

    pub async fn observe_startup_with_environment(
        &self,
        additional_environment: Vec<(String, String)>,
    ) -> StartupObservation {
        let mut binding = ExternalProviderLaunchBinding::claude(
            self.adapter_executable.clone(),
            vec![self.bundled_executable.to_string_lossy().into_owned()],
        )
        .expect("fixture Claude binding");
        binding.launch.environment = vec![
            (
                "PATH".to_owned(),
                self.global_directory.to_string_lossy().into_owned(),
            ),
            // Prevent an inherited owner executable from entering this fixture.
            // The common composer must replace this empty selection with global PATH truth.
            ("CLAUDE_CODE_EXECUTABLE".to_owned(), String::new()),
            (
                "GLOBAL_HARNESS_FIXTURE_ADAPTER_RECEIPT".to_owned(),
                self.adapter_dispatch_receipt.to_string_lossy().into_owned(),
            ),
            (
                "GLOBAL_HARNESS_FIXTURE_NATIVE_RECEIPT".to_owned(),
                self.native_execution_receipt.to_string_lossy().into_owned(),
            ),
        ];
        binding.launch.environment.extend(additional_environment);
        let runtime = CollaborationRuntime::start_with_external_providers(
            CollaborationRuntimeInputs {
                directory: self.runtime_directory.clone(),
                codex_home: self.runtime_directory.clone(),
                backend_socket: self.runtime_directory.join("backend.sock"),
                mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                native_schema: None,
                peer_registry_directory: None,
                remote_control_server_name: None,
                owner_human_id: None,
            },
            vec![ExternalProviderStartup::Launch(binding)],
        )
        .await
        .expect("real common startup and provider storage");
        let client =
            CollaborationClient::connect(&self.runtime_directory, "global-harness-proof", "1")
                .await
                .expect("real endpoint inventory client");
        let inventory = client
            .list_endpoints()
            .await
            .expect("real endpoint inventory");
        let endpoint = inventory
            .endpoints
            .iter()
            .find(|endpoint| String::from(endpoint.endpoint.endpoint_id.clone()) == "claude-local")
            .expect("Claude endpoint disposition");
        let availability = endpoint.availability.clone();
        let published_channels = endpoint.channels.clone();
        drop(client);
        runtime
            .shutdown()
            .await
            .expect("owned runtime and adapter shutdown");
        let adapter_dispatch = self.adapter_dispatch_receipt.exists().then(|| {
            serde_json::from_slice(
                &fs::read(&self.adapter_dispatch_receipt).expect("adapter receipt"),
            )
            .expect("typed adapter dispatch receipt")
        });
        let native_execution = self.native_execution_receipt.exists().then(|| {
            serde_json::from_slice(
                &fs::read(&self.native_execution_receipt).expect("native receipt"),
            )
            .expect("typed real native execution receipt")
        });
        eprintln!(
            "global harness process receipts: availability={availability:?}; adapter={adapter_dispatch:?}; native={native_execution:?}"
        );
        StartupObservation {
            availability,
            adapter_dispatch,
            native_execution,
            published_channels,
        }
    }
}
