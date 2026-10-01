//! CLI token export commands backed by the shared secret-store token service.

pub use codex_router_secret_store::local_router_token::LocalRouterTokenError as TokenCommandError;
pub use codex_router_secret_store::local_router_token::LocalRouterTokenService;

/// Shell dialect for token export.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shell {
    /// POSIX-compatible shell assignment.
    Posix,
}

/// Renders a shell export for the local router token.
#[must_use]
pub fn export_token_assignment(env_var: &str, token: &str, shell: Shell) -> String {
    match shell {
        Shell::Posix => format!("export {env_var}={}\n", quote_posix(token)),
    }
}

fn quote_posix(value: &str) -> String {
    let mut quoted = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}
