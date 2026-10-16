//! The single validated HTTPS authority forwarded by an opted-in private proxy.

use std::{io, num::NonZeroU16};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyAuthority {
    node_name: String,
    https_port: NonZeroU16,
}

impl ProxyAuthority {
    pub fn new(node_name: String, https_port: NonZeroU16) -> io::Result<Self> {
        if node_name.len() > 253
            || !node_name.contains('.')
            || node_name.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            || node_name.parse::<std::net::IpAddr>().is_ok()
        {
            return Err(io::Error::other(
                "proxy authority requires a valid node DNS name",
            ));
        }
        Ok(Self {
            node_name: node_name.to_ascii_lowercase(),
            https_port,
        })
    }

    #[must_use]
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    #[must_use]
    pub const fn https_port(&self) -> NonZeroU16 {
        self.https_port
    }

    #[must_use]
    pub fn authority(&self) -> String {
        format!("{}:{}", self.node_name, self.https_port)
    }
}

pub(crate) async fn normalize_default_https_host(
    axum::extract::State(authority): axum::extract::State<ProxyAuthority>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if authority.https_port.get() == 443
        && request
            .headers()
            .get(http::header::HOST)
            .and_then(|host| host.to_str().ok())
            .is_some_and(|host| host.eq_ignore_ascii_case(authority.node_name()))
    {
        match http::HeaderValue::try_from(authority.authority()) {
            Ok(host) => {
                request.headers_mut().insert(http::header::HOST, host);
            }
            Err(_) => {
                use axum::response::IntoResponse as _;
                return http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }
    next.run(request).await
}
