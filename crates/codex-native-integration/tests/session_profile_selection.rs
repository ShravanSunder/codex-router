use std::ffi::OsString;
use std::path::Path;

use codex_native_integration::SessionLaunch;
use codex_native_integration::SessionProfile;

#[test]
fn debug_profile_selection_preserves_every_native_launch_tail() {
    // Arrange: the same caller arguments exercise hosted and local launch paths.
    let socket = Path::new("/tmp/debug-owner/backend.sock");
    let cwd = Path::new("/tmp/debug-worktree");
    let arguments = vec![OsString::from("--model"), OsString::from("example-model")];
    let launches = [
        SessionLaunch::new(socket, cwd, &arguments),
        SessionLaunch::resume(socket, cwd, &arguments, "thread-id"),
        SessionLaunch::fork(socket, cwd, &arguments, "thread-id"),
        SessionLaunch::local(cwd, &arguments),
        SessionLaunch::resume_local(cwd, &arguments, "thread-id"),
        SessionLaunch::fork_local(cwd, &arguments, "thread-id"),
    ];
    for launch in launches {
        let original = launch.arguments();
        // Act: profile selection changes only the launcher-owned profile.
        let debug = launch.with_profile(SessionProfile::RouterDebug).arguments();
        // Assert: native arguments, exact target and cwd remain identical.
        let mut expected = original.into_iter();
        assert_eq!(expected.next(), Some(OsString::from("--profile")));
        assert_eq!(expected.next(), Some(OsString::from("codex-router")));
        let expected: Vec<_> = [
            OsString::from("--profile"),
            OsString::from("codex-router-debug"),
        ]
        .into_iter()
        .chain(expected)
        .collect();
        assert_eq!(debug, expected);
    }
}
