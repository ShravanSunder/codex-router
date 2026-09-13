use std::{ffi::OsString, net::Ipv4Addr, path::Path};

pub fn automation_proof_startup_arguments(
    run_directory: &Path,
) -> Result<Vec<OsString>, Box<dyn std::error::Error>> {
    let canonical_run_directory = run_directory.canonicalize()?;
    let control_socket = canonical_run_directory.join("agent-communication/control.sock");
    let control_socket = control_socket
        .to_str()
        .ok_or("acceptance Control socket path is not UTF-8")?;

    let reservation = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let proxy_port = reservation.local_addr()?.port();

    let mut permission_network = toml::Table::new();
    permission_network.insert("enabled".to_owned(), toml::Value::Boolean(true));
    let mut permission_profile = toml::Table::new();
    permission_profile.insert(
        "extends".to_owned(),
        toml::Value::String(":workspace".to_owned()),
    );
    permission_profile.insert("network".to_owned(), toml::Value::Table(permission_network));

    let mut unix_sockets = toml::Table::new();
    unix_sockets.insert(
        control_socket.to_owned(),
        toml::Value::String("allow".to_owned()),
    );
    let mut network_proxy = toml::Table::new();
    network_proxy.insert("enabled".to_owned(), toml::Value::Boolean(true));
    network_proxy.insert(
        "proxy_url".to_owned(),
        toml::Value::String(format!("http://127.0.0.1:{proxy_port}")),
    );
    for setting in [
        "enable_socks5",
        "allow_upstream_proxy",
        "allow_local_binding",
        "credential_broker",
        "dangerously_allow_all_unix_sockets",
    ] {
        network_proxy.insert(setting.to_owned(), toml::Value::Boolean(false));
    }
    network_proxy.insert("domains".to_owned(), toml::Value::Table(toml::Table::new()));
    network_proxy.insert("unix_sockets".to_owned(), toml::Value::Table(unix_sockets));

    drop(reservation);
    Ok(vec![
        OsString::from("-c"),
        OsString::from(format!(
            "permissions.automation-proof={}",
            toml::Value::Table(permission_profile)
        )),
        OsString::from("-c"),
        OsString::from(format!(
            "features.network_proxy={}",
            toml::Value::Table(network_proxy)
        )),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::DirBuilderExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestRunDirectory(PathBuf);

    impl TestRunDirectory {
        fn create(mode: &str) -> Result<Self, Box<dyn std::error::Error>> {
            let root = PathBuf::from("/tmp").join(format!(
                "automation-debug-host-permissions-{mode}-{}-{}",
                std::process::id(),
                NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new().mode(0o700).create(&root)?;
            Ok(Self(root))
        }
    }

    impl Drop for TestRunDirectory {
        fn drop(&mut self) {
            let _cleanup = std::fs::remove_dir_all(&self.0);
        }
    }

    fn parse_override(argument: &OsString) -> Result<toml::Value, Box<dyn std::error::Error>> {
        let argument = argument.to_str().ok_or("override is not UTF-8")?;
        let (_, value) = argument.split_once('=').ok_or("override has no value")?;
        let mut parsed: toml::Table = toml::from_str(&format!("value={value}"))?;
        parsed
            .remove("value")
            .ok_or_else(|| "parsed override has no value".into())
    }

    fn assert_exact_permissions(
        run_directory: &Path,
        arguments: &[OsString],
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(arguments.len(), 4);
        assert_eq!(arguments[0], "-c");
        assert_eq!(arguments[2], "-c");
        assert!(
            arguments[1]
                .to_str()
                .is_some_and(|argument| argument.starts_with("permissions.automation-proof="))
        );
        assert!(
            arguments[3]
                .to_str()
                .is_some_and(|argument| argument.starts_with("features.network_proxy="))
        );

        let profile = parse_override(&arguments[1])?;
        assert_eq!(profile["extends"].as_str(), Some(":workspace"));
        assert_eq!(profile["network"]["enabled"].as_bool(), Some(true));
        assert_eq!(profile["network"].as_table().map(toml::Table::len), Some(1));
        assert_eq!(profile.as_table().map(toml::Table::len), Some(2));

        let proxy = parse_override(&arguments[3])?;
        assert_eq!(proxy.as_table().map(toml::Table::len), Some(9));
        assert_eq!(proxy["enabled"].as_bool(), Some(true));
        assert_eq!(proxy["enable_socks5"].as_bool(), Some(false));
        assert_eq!(proxy["allow_upstream_proxy"].as_bool(), Some(false));
        assert_eq!(proxy["allow_local_binding"].as_bool(), Some(false));
        assert_eq!(proxy["credential_broker"].as_bool(), Some(false));
        assert_eq!(
            proxy["dangerously_allow_all_unix_sockets"].as_bool(),
            Some(false)
        );
        assert_eq!(proxy["domains"].as_table().map(toml::Table::len), Some(0));
        let unix_sockets = proxy["unix_sockets"]
            .as_table()
            .ok_or("unix_sockets is not a table")?;
        assert_eq!(unix_sockets.len(), 1);
        let socket = run_directory
            .canonicalize()?
            .join("agent-communication/control.sock");
        assert_eq!(
            unix_sockets.get(socket.to_str().ok_or("socket path is not UTF-8")?),
            Some(&toml::Value::String("allow".to_owned()))
        );
        let proxy_url = proxy["proxy_url"]
            .as_str()
            .ok_or("proxy_url is not a string")?;
        let proxy_port = proxy_url
            .strip_prefix("http://127.0.0.1:")
            .ok_or("proxy_url is not loopback HTTP")?
            .parse::<u16>()?;
        assert_ne!(proxy_port, 0);
        Ok(())
    }

    #[test]
    fn fresh_and_resume_launches_receive_the_same_scoped_permission_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        for mode in ["fresh", "resume"] {
            let run = TestRunDirectory::create(mode)?;
            let arguments = automation_proof_startup_arguments(&run.0)?;
            assert_exact_permissions(&run.0, &arguments)?;
        }
        Ok(())
    }
}
