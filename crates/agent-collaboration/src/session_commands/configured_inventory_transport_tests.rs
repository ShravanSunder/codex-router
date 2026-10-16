//! Real HTTP redirects and TLS verification; native catalog data remains a fixture.
use super::*;
use crate::presentation::session_picker::PickerSourceContext;
use crate::sessions::configured_inventory_read_tests::SourceApiFixture;
use crate::sessions::router_connection_registry::RouterConnectionRegistry;
use std::{net::Ipv4Addr, path::Path, sync::Arc};
use tokio::net::{TcpListener, TcpStream};

const CERTIFICATE: &[u8] =
    include_bytes!("test_support/configured_tls_fixture/localhost-certificate.der");
const PUBLIC_TEST_KEY: &[u8] =
    include_bytes!("test_support/configured_tls_fixture/public-test-key.der");
const CHILD_PROBE: &str = "sessions::configured_inventory_transport::tests::https_source_probe";

fn profile_for_address(address: &str) -> RouterConnectionProfile {
    let mut registry = RouterConnectionRegistry::parse(
        &serde_json::json!({"version":1,"routers":[{
            "name":"TLS boundary fixture","connection":{
                "kind":"remote","serviceId":"00000000-0000-4000-8000-000000000001",
                "mcpUrl":address
            }
        }]})
        .to_string(),
    )
    .unwrap();
    registry.routers.remove(0)
}

#[tokio::test]
async fn http_source_redirect_cannot_upgrade_or_contact_https_destination() {
    let destination = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let location = format!(
        "https://localhost:{}/mcp",
        destination.local_addr().unwrap().port()
    );
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let profile = profile_for_address(&format!("http://{}/mcp", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        let router = axum::Router::new().route(
            "/mcp",
            axum::routing::post(move || {
                let location = location.clone();
                async move {
                    (
                        axum::http::StatusCode::FOUND,
                        [(axum::http::header::LOCATION, location)],
                    )
                }
            }),
        );
        axum::serve(listener, router).await.unwrap();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        ConfiguredInventoryClient::new(&profile)
            .unwrap()
            .inventory(),
    )
    .await
    .unwrap();
    assert!(
        matches!(result, Err(ClientError::Rejected { code: 302, .. })),
        "redirect must be returned as source rejection"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(25), destination.accept())
            .await
            .is_err(),
        "redirect contacted the HTTPS destination"
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn https_source_preserves_certificate_verification_and_trusted_api_reads() {
    let source =
        SourceApiFixture::start("00000000-0000-4000-8000-000000000001", "fixture-model").await;
    let PickerSourceContext::ConfiguredHosted(source_profile) =
        source.request("TLS source").source_context
    else {
        panic!("configured fixture")
    };
    let crate::sessions::router_connection_registry::RouterConnectionTransport::Remote {
        mcp_address,
        ..
    } = &source_profile.transport
    else {
        panic!("HTTP fixture")
    };
    let upstream = url::Url::parse(mcp_address.as_str()).unwrap();
    let upstream_address = (
        upstream.host_str().unwrap().to_owned(),
        upstream.port().unwrap(),
    );
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = format!(
        "https://localhost:{}/mcp",
        listener.local_addr().unwrap().port()
    );
    let configuration = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                CERTIFICATE.to_vec(),
            )],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                PUBLIC_TEST_KEY.to_vec(),
            )),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(configuration));
    let relay = tokio::spawn(async move {
        for expected_trust in [false, true] {
            let (stream, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(stream).await;
            if !expected_trust {
                assert!(tls.is_err(), "untrusted TLS certificate was accepted");
            } else {
                let mut tls = tls.expect("trusted HTTPS handshake");
                let mut upstream = TcpStream::connect(&upstream_address).await.unwrap();
                // The isolated probe process drops its pooled connection at exit.
                // A close without close_notify is allowed only after the verified
                // handshake; the parent still requires the actual API result.
                if let Err(error) = tokio::io::copy_bidirectional(&mut tls, &mut upstream).await {
                    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
                }
            }
        }
    });
    let empty_roots = tempfile::NamedTempFile::new().unwrap();
    for (trusted, roots) in [
        (false, empty_roots.path().to_path_buf()),
        (true, Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session_commands/test_support/configured_tls_fixture/localhost-certificate.pem")),
    ] {
        let output = tokio::time::timeout(Duration::from_secs(5),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD_PROBE, "--ignored", "--nocapture"])
                .env("SSL_CERT_FILE", roots)
                .env_remove("SSL_CERT_DIR")
                .env("ROUTER_TLS_FIXTURE_ADDRESS", &address)
                .env("ROUTER_TLS_FIXTURE_EXPECT_TRUST", if trusted { "yes" } else { "no" })
                .kill_on_drop(true).output(),
        ).await.unwrap().unwrap();
        assert!(output.status.success(), "isolated HTTPS probe failed: {}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"), "exact HTTPS probe must execute");
    }
    tokio::time::timeout(Duration::from_secs(2), relay)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        source.api_call_count("endpoints_list"),
        1,
        "only verified HTTPS can reach the genuine API"
    );
    source.finish().await;
}

#[tokio::test]
#[ignore = "executed by the parent TLS boundary test in an isolated trust environment"]
async fn https_source_probe() {
    let address = std::env::var("ROUTER_TLS_FIXTURE_ADDRESS").unwrap();
    let trusted = std::env::var("ROUTER_TLS_FIXTURE_EXPECT_TRUST").unwrap() == "yes";
    let result = ConfiguredInventoryClient::new(&profile_for_address(&address))
        .unwrap()
        .inventory()
        .await;
    if trusted {
        assert!(result.is_ok(), "verified HTTPS must read the genuine API");
    } else {
        assert!(
            matches!(result, Err(ClientError::Transport(_))),
            "untrusted certificate must fail the real transport"
        );
    }
}

#[test]
fn ssh_source_read_failure_preserves_unavailable_and_invalid_categories() {
    use crate::sessions::source_ssh_protocol::{SourceReadError, SourceRejection};

    // Availability failures must reach the loader as SourceUnavailable, as HTTP does.
    for failure in [
        SourceReadError::ReadUnavailable,
        SourceReadError::Rejected(SourceRejection::SourceUnavailable),
    ] {
        assert!(
            matches!(source_read_client_error(failure), ClientError::Transport(_)),
            "SSH reachability failure was mislabeled invalid inventory: {failure}"
        );
    }
    // A reachable wrong service or malformed response must never become empty/unavailable.
    for failure in [
        SourceReadError::InvalidReply,
        SourceReadError::Rejected(SourceRejection::WrongService),
        SourceReadError::Rejected(SourceRejection::InvalidRequest),
    ] {
        assert!(matches!(
            source_read_client_error(failure),
            ClientError::Protocol(_)
        ));
    }
}
