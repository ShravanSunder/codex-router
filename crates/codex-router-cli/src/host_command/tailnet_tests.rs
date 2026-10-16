//! Literal CLI scenarios from the private Tailnet MCP contract, R1 and R7.

use std::ffi::OsString;

use super::{HostAction, HostCommand};

fn parse_host(arguments: &[&str]) -> Result<HostCommand, String> {
    HostCommand::parse(arguments.iter().map(OsString::from).collect())
}

#[test]
fn tailnet_access_accepts_the_mode_and_normalizes_the_default_https_port() {
    let implicit_port = parse_host(&["--remote-access", "tailscale"])
        .expect("R1: opt-in Tailscale access is a valid Host launch");
    let explicit_port = parse_host(&[
        "--remote-access",
        "tailscale",
        "--tailscale-https-port",
        "8443",
    ])
    .expect("R1: the literal default port is valid");

    assert!(implicit_port.runs_foreground());
    assert_eq!(implicit_port, explicit_port);
    assert_ne!(
        implicit_port,
        parse_host(&[]).expect("existing local-only launch"),
        "the opt-in must survive parsing"
    );
}

#[test]
fn tailnet_access_retains_a_selected_nonzero_https_port() {
    let default_port =
        parse_host(&["--remote-access", "tailscale"]).expect("Tailscale access with default port");
    for selected_port in ["9443", "65535"] {
        let selected = parse_host(&[
            "--remote-access",
            "tailscale",
            "--tailscale-https-port",
            selected_port,
        ])
        .expect("R1: a selected nonzero HTTPS port is valid");

        assert_ne!(selected, default_port, "port {selected_port} was discarded");
    }
}

#[test]
fn tailnet_access_options_can_accompany_the_existing_restart_command() {
    let command = parse_host(&[
        "restart",
        "--remote-access",
        "tailscale",
        "--tailscale-https-port",
        "9443",
    ])
    .expect("global Host options remain valid after the action");

    assert_eq!(command.action(), HostAction::Restart);
    assert!(!command.runs_foreground());
}

#[test]
fn tailnet_access_rejects_unsupported_modes_and_invalid_port_combinations() {
    let rejected_arguments: &[&[&str]] = &[
        &["--remote-access", "funnel"],
        &["--remote-access", "lan"],
        &["--remote-access"],
        &["--tailscale-https-port", "9443"],
        &[
            "--remote-access",
            "tailscale",
            "--tailscale-https-port",
            "0",
        ],
        &[
            "--remote-access",
            "tailscale",
            "--tailscale-https-port",
            "65536",
        ],
        &[
            "--remote-access",
            "tailscale",
            "--tailscale-https-port",
            "-1",
        ],
        &["--remote-access", "tailscale", "--tailscale-https-port"],
    ];

    for arguments in rejected_arguments {
        assert!(
            parse_host(arguments).is_err(),
            "R1 requires rejecting {arguments:?} before launch"
        );
    }
}
