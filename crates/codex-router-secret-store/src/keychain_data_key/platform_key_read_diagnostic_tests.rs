use super::map_platform_key_read_result;
use crate::keychain_data_key::{KeychainAccessError, synthetic_platform_key_error};
use std::io::Write;
use std::sync::{Arc, Mutex};

static LOG_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn returned_platform_failures_emit_static_category_and_keep_unavailable_result() {
    // Literal statuses and obligations come from the SDK SecBase.h, not the mapper.
    let cases = [
        (-25308, "interaction_not_allowed"),
        (-25315, "interaction_required"),
        (-25293, "auth_failed"),
        (-128, "user_canceled"),
        (-25291, "keychain_not_available"),
        (-25294, "no_such_keychain"),
        (-25295, "invalid_keychain"),
        (-25307, "no_default_keychain"),
        (-25316, "key_data_not_available"),
        (-34018, "missing_entitlement"),
    ];
    for storage_access in [false, true] {
        for (code, expected) in cases {
            let (output, result) = capture_log_output(|| {
                map_platform_key_read_result(Err(synthetic_platform_key_error(
                    code,
                    storage_access,
                )))
            });
            assert_eq!(result, Err(KeychainAccessError::Unavailable));
            assert!(
                output.contains(&format!("error.kind=\"{expected}\"")),
                "{output}"
            );
            assert_eq!(
                output
                    .matches("pooled credential Keychain read failed")
                    .count(),
                1
            );
            assert!(!output.contains(&code.to_string()));
        }
    }
}

#[test]
fn successful_or_absent_key_results_stay_unchanged_and_emit_no_failure() {
    let secret = b"private_key_bytes_canary".to_vec();
    let (output, result) = capture_log_output(|| map_platform_key_read_result(Ok(secret.clone())));
    assert_eq!(result, Ok(Some(secret)));
    assert!(output.is_empty());
    let (output, result) =
        capture_log_output(|| map_platform_key_read_result(Err(keyring_core::Error::NoEntry)));
    assert_eq!(result, Ok(None));
    assert!(output.is_empty());
}

#[test]
fn unknown_platform_and_secret_bearing_errors_never_emit_payloads() {
    let cases = [
        (
            synthetic_platform_key_error(-987654321, false),
            "platform_failure_unclassified",
        ),
        (
            synthetic_platform_key_error(-987654321, true),
            "storage_access_unclassified",
        ),
        (
            keyring_core::Error::PlatformFailure(Box::new(std::io::Error::other(
                "private_platform_message_canary",
            ))),
            "platform_failure_unclassified",
        ),
        (
            keyring_core::Error::NoStorageAccess(Box::new(std::io::Error::other(
                "private_storage_message_canary",
            ))),
            "storage_access_unclassified",
        ),
        (
            keyring_core::Error::BadEncoding(b"private_key_bytes_canary".to_vec()),
            "invalid_key_encoding",
        ),
        (
            keyring_core::Error::BadDataFormat(
                b"private_key_bytes_canary".to_vec(),
                Box::new(std::io::Error::other("private_platform_message_canary")),
            ),
            "invalid_key_data",
        ),
        (
            keyring_core::Error::BadStoreFormat("private_store_message_canary".to_owned()),
            "invalid_store_format",
        ),
        (
            keyring_core::Error::Invalid(
                "private_locator_canary".to_owned(),
                "private_locator_message_canary".to_owned(),
            ),
            "invalid_locator",
        ),
        (
            keyring_core::Error::NoDefaultStore,
            "keyring_failure_unclassified",
        ),
    ];
    for (error, expected) in cases {
        let (output, result) = capture_log_output(|| map_platform_key_read_result(Err(error)));
        assert_eq!(result, Err(KeychainAccessError::Unavailable));
        assert!(
            output.contains(&format!("error.kind=\"{expected}\"")),
            "{output}"
        );
        assert_eq!(
            output
                .matches("pooled credential Keychain read failed")
                .count(),
            1
        );
        assert!(!output.contains("canary"));
        assert!(!output.contains("987654321"));
        assert!(!output.contains("PlatformFailure"));
        assert!(!output.contains("NoStorageAccess"));
    }
}

fn capture_log_output<TOutput>(emit: impl FnOnce() -> TOutput) -> (String, TOutput) {
    let _guard = LOG_CAPTURE_LOCK.lock().expect("log capture lock");
    let captured = CapturedDiagnosticWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(captured.clone())
        .finish();
    let result = tracing::subscriber::with_default(subscriber, emit);
    let bytes = captured.bytes.lock().expect("captured log lock").clone();
    (
        String::from_utf8(bytes).expect("captured UTF-8 logs"),
        result,
    )
}

#[derive(Clone, Default)]
struct CapturedDiagnosticWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedDiagnosticWriter {
    type Writer = CapturedDiagnosticWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for CapturedDiagnosticWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let mut bytes = self
            .bytes
            .lock()
            .map_err(|_| std::io::Error::other("log capture lock poisoned"))?;
        bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
