use std::ffi::OsString;
use std::path::PathBuf;

use codex_router_codex::DesktopLaunchPolicyCommand;

#[test]
fn desktop_launch_policy_projects_the_native_launchctl_contract() {
    let command = DesktopLaunchPolicyCommand::new(PathBuf::from("/bin/launchctl"));

    assert_eq!(command.executable(), PathBuf::from("/bin/launchctl"));
    assert_eq!(
        command.arguments(),
        vec![
            OsString::from("setenv"),
            OsString::from("CODEX_APP_SERVER_USE_LOCAL_DAEMON"),
            OsString::from("1"),
        ]
    );
}
