use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::MutexGuard;

use security_framework::os::macos::keychain::CreateOptions;
use security_framework::os::macos::keychain::KeychainSettings;
use security_framework::os::macos::keychain::KeychainUserInteractionLock;
use security_framework::os::macos::keychain::SecKeychain;
use tempfile::TempDir;
use zeroize::Zeroizing;

use super::KeychainAccess;
use super::KeychainAccessError;
use super::PooledCredentialDataKey;
use super::ROUTER_KEYCHAIN_SERVICE;
use crate::backend::SecretStore;

static TEMPORARY_KEYCHAIN_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn acquire_temporary_keychain_test_lock() -> MutexGuard<'static, ()> {
    match TEMPORARY_KEYCHAIN_TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct TemporaryKeychainDirectoryGuard {
    directory: Option<TempDir>,
    path: PathBuf,
}

impl TemporaryKeychainDirectoryGuard {
    fn new() -> Result<Self, std::io::Error> {
        let directory = TempDir::new_in("/private/tmp")?;
        let path = directory.path().to_path_buf();
        Ok(Self {
            directory: Some(directory),
            path,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryKeychainDirectoryGuard {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take()
            && let Err(error) = directory.close()
        {
            eprintln!("temporary Keychain test directory cleanup failed: {error}");
        }
    }
}

/// Explicit path-specific Keychain used only by this crate's macOS tests.
pub(crate) struct TemporaryKeychainAccess {
    keychain: SecKeychain,
    _interaction_lock: KeychainUserInteractionLock,
    password: Zeroizing<String>,
    path: PathBuf,
    _directory: TemporaryKeychainDirectoryGuard,
}

impl TemporaryKeychainAccess {
    pub(crate) fn new() -> Result<Self, std::io::Error> {
        let interaction_lock = SecKeychain::disable_user_interaction()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let directory = TemporaryKeychainDirectoryGuard::new()?;
        let path = directory.path().join("router-test.keychain");
        let password = Zeroizing::new(format!("router-test-{}", uuid::Uuid::now_v7()));
        let mut keychain = CreateOptions::new()
            .password(password.as_str())
            .prompt_user(false)
            .create(&path)
            .map_err(|error| {
                std::io::Error::other(format!(
                    "temporary Keychain creation failed: {error} ({}) at {}",
                    error.code(),
                    path.display()
                ))
            })?;
        keychain
            .unlock(Some(password.as_str()))
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let mut settings = KeychainSettings::new();
        settings.set_lock_on_sleep(false);
        settings.set_lock_interval(None);
        keychain
            .set_settings(&settings)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Self {
            keychain,
            _interaction_lock: interaction_lock,
            password,
            path,
            _directory: directory,
        })
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn password(&self) -> &str {
        self.password.as_str()
    }
}

impl KeychainAccess for TemporaryKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        match self.keychain.find_generic_password(service, account) {
            Ok((secret, _item)) => Ok(Some(secret.to_owned())),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(_) => Err(KeychainAccessError::Unavailable),
        }
    }

    fn add_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        self.keychain
            .add_generic_password(service, account, secret)
            .map_err(|_| KeychainAccessError::Unavailable)
    }
}

fn compile_keychain_acl_probe(
    directory: &Path,
    build_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let source_path = directory.join(format!("keychain-acl-probe-{build_id}.rs"));
    let executable_path = directory.join(format!("keychain-acl-probe-{build_id}"));
    let source = r#"
use security_framework::os::macos::keychain::SecKeychain;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::keychain_data_key::PooledCredentialDataKey;
use codex_router_secret_store::model::SecretKey;
use std::process::ExitCode;

#[cfg(build_a)] const BUILD_MARKER: u8 = 1;
#[cfg(build_b)] const BUILD_MARKER: u8 = 2;
const EXIT_KEYCHAIN_ACCESS_DENIED: u8 = 41;

fn main() -> ExitCode {
    let arguments = std::env::args().collect::<Vec<_>>();
    if BUILD_MARKER == 0 {
        return ExitCode::from(20);
    }
    let mode = &arguments[1];
    let _interaction_lock = match SecKeychain::disable_user_interaction() {
        Ok(lock) => lock,
        Err(_) => return ExitCode::from(23),
    };
    if !matches!(SecKeychain::user_interaction_allowed(), Ok(false)) {
        return ExitCode::from(24);
    }

    if mode == "decrypt" {
        if arguments.len() != 8 {
            return ExitCode::from(20);
        }
        let keychain_path = &arguments[2];
        let keychain_password = &arguments[3];
        let service = &arguments[4];
        let keychain_account = &arguments[5];
        let secret_root = &arguments[6];
        let credential_name = &arguments[7];
        let mut keychain = match SecKeychain::open(keychain_path) {
            Ok(keychain) => keychain,
            Err(_) => return ExitCode::from(21),
        };
        if keychain.unlock(Some(keychain_password)).is_err() {
            return ExitCode::from(22);
        }
        let (key_bytes, _item) = match keychain.find_generic_password(service, keychain_account) {
            Ok(item) => item,
            Err(error) if matches!(error.code(), -25308 | -25293) => {
                eprintln!("keychain item access denied with OSStatus {}", error.code());
                return ExitCode::from(EXIT_KEYCHAIN_ACCESS_DENIED);
            }
            Err(error) => {
                eprintln!("keychain item lookup failed with OSStatus {}", error.code());
                return ExitCode::from(27);
            }
        };
        if key_bytes.len() != 32 {
            return ExitCode::from(34);
        }
        let mut data_key = [0_u8; 32];
        data_key.copy_from_slice(&key_bytes);
        let file_store = match FileSecretStore::open(secret_root) {
            Ok(file_store) => file_store,
            Err(_) => return ExitCode::from(29),
        };
        let key = match SecretKey::new(credential_name.clone()) {
            Ok(key) => key,
            Err(_) => return ExitCode::from(30),
        };
        let encrypted_store = EncryptedCredentialStore::new(
            file_store,
            PooledCredentialDataKey::from_bytes(data_key),
        );
        return match encrypted_store.read_secret(&key) {
            Ok(_) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(33),
        };
    }

    if arguments.len() != 7 {
        return ExitCode::from(20);
    }
    let keychain_path = &arguments[2];
    let keychain_password = &arguments[3];
    let service = &arguments[4];
    let account = &arguments[5];
    let secret_hex = &arguments[6];

    let mut keychain = match SecKeychain::open(keychain_path) {
        Ok(keychain) => keychain,
        Err(_) => return ExitCode::from(21),
    };
    if keychain.unlock(Some(keychain_password)).is_err() {
        return ExitCode::from(22);
    }

    if mode == "write" {
        let mut secret = Vec::new();
        for pair in secret_hex.as_bytes().chunks_exact(2) {
            let text = match std::str::from_utf8(pair) {
                Ok(text) => text,
                Err(_) => return ExitCode::from(24),
            };
            match u8::from_str_radix(text, 16) {
                Ok(byte) => secret.push(byte),
                Err(_) => return ExitCode::from(25),
            }
        }
        return match keychain.add_generic_password(service, account, &secret) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(26),
        };
    }
    if mode == "read" {
        return match keychain.find_generic_password(service, account) {
            Ok((_secret, _item)) => ExitCode::SUCCESS,
            Err(error) if matches!(error.code(), -25308 | -25293) => {
                eprintln!("keychain item access denied with OSStatus {}", error.code());
                ExitCode::from(EXIT_KEYCHAIN_ACCESS_DENIED)
            }
            Err(error) => {
                eprintln!("keychain item lookup failed with OSStatus {}", error.code());
                ExitCode::from(27)
            }
        };
    }
    ExitCode::from(28)
}
"#;
    fs::write(&source_path, source)?;
    let executable = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let current_executable = std::env::current_exe()?;
    let dependencies = current_executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("test executable should have a parent"))?;
    let framework_library = fs::read_dir(&dependencies)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("libsecurity_framework-") && name.ends_with(".rlib")
                })
        })
        .ok_or_else(|| std::io::Error::other("security-framework Rust library is missing"))?;
    let secret_store_library = fs::read_dir(&dependencies)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("libcodex_router_secret_store-") && name.ends_with(".rlib")
                })
        })
        .ok_or_else(|| {
            std::io::Error::other("codex-router-secret-store Rust library is missing")
        })?;
    let build_cfg = format!("build_{build_id}");
    let output = Command::new(executable)
        .arg("--edition=2024")
        .arg("--cfg")
        .arg(build_cfg)
        .arg(&source_path)
        .arg("--extern")
        .arg(format!(
            "security_framework={}",
            framework_library.display()
        ))
        .arg("--extern")
        .arg(format!(
            "codex_router_secret_store={}",
            secret_store_library.display()
        ))
        .arg("-L")
        .arg(format!("dependency={}", dependencies.display()))
        .arg("-o")
        .arg(&executable_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "temporary ACL probe should compile: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
        .into());
    }
    Ok(executable_path)
}

fn run_keychain_acl_probe(
    executable: &Path,
    mode: &str,
    keychain_path: &Path,
    keychain_password: &str,
    service: &str,
    account: &str,
    secret_hex: &str,
) -> Result<std::process::ExitStatus, std::io::Error> {
    Command::new(executable)
        .arg(mode)
        .arg(keychain_path)
        .arg(keychain_password)
        .arg(service)
        .arg(account)
        .arg(secret_hex)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
}

fn run_keychain_decrypt_probe(
    executable: &Path,
    keychain_path: &Path,
    keychain_password: &str,
    service: &str,
    keychain_account: &str,
    secret_root: &Path,
    credential_name: &str,
) -> Result<std::process::Output, std::io::Error> {
    Command::new(executable)
        .arg("decrypt")
        .arg(keychain_path)
        .arg(keychain_password)
        .arg(service)
        .arg(keychain_account)
        .arg(secret_root)
        .arg(credential_name)
        .output()
}

#[test]
fn temporary_keychain_probe_builds_differently_marked_binaries() {
    let build_directory =
        TempDir::new_in("/private/tmp").expect("temporary build directory should create");
    let first_build = compile_keychain_acl_probe(build_directory.path(), "a")
        .expect("first ACL probe should compile");
    let second_build = compile_keychain_acl_probe(build_directory.path(), "b")
        .expect("second ACL probe should compile");

    assert_ne!(
        fs::read(first_build).expect("first build bytes should read"),
        fs::read(second_build).expect("second build bytes should read"),
        "the ACL probe executables must be differently built binaries"
    );
}

#[test]
#[ignore = "requires the opt-in unsandboxed temporary-Keychain ACL test command"]
fn temporary_keychain_denies_an_unapproved_second_build_without_interaction() {
    assert!(matches!(
        std::env::var("CODEX_ROUTER_KEYCHAIN_TESTS").as_deref(),
        Ok("1")
    ));
    let _keychain_test_lock = acquire_temporary_keychain_test_lock();
    let build_directory =
        TempDir::new_in("/private/tmp").expect("temporary build directory should create");
    let first_build = compile_keychain_acl_probe(build_directory.path(), "a")
        .expect("first ACL probe should compile");
    let second_build = compile_keychain_acl_probe(build_directory.path(), "b")
        .expect("second ACL probe should compile");
    assert_ne!(
        fs::read(&first_build).expect("first build bytes should read"),
        fs::read(&second_build).expect("second build bytes should read"),
        "the ACL probe executables must be differently built binaries"
    );

    let keychain = TemporaryKeychainAccess::new().expect("temporary Keychain file");
    let account = "pooled-credential-key:pre-release-two-build";
    let secret_hex = "3737373737373737373737373737373737373737373737373737373737373737";
    let created = run_keychain_acl_probe(
        &first_build,
        "write",
        keychain.path(),
        keychain.password(),
        super::ROUTER_KEYCHAIN_SERVICE,
        account,
        secret_hex,
    )
    .expect("first build should start");
    assert!(created.success(), "first build should create its item");
    let secret_root = TempDir::new().expect("temporary encrypted credential root");
    let file_store = crate::file_backend::FileSecretStore::open(secret_root.path())
        .expect("encrypted file store should open");
    let encrypted_store = crate::encrypted_credential_store::EncryptedCredentialStore::new(
        file_store,
        PooledCredentialDataKey::from_bytes([0x37; 32]),
    );
    let credential_id = codex_router_core::ids::AccountId::new("acct_two_build_acl")
        .expect("credential account id");
    let credential_key =
        crate::account_tokens::openai_account_credential_bundle_key(&credential_id, 1)
            .expect("credential key");
    let plaintext = "two-build-acl-token-canary";
    encrypted_store
        .write_secret(
            &credential_key,
            &codex_router_core::redaction::SecretString::new(plaintext),
        )
        .expect("credential should be encrypted");
    let encrypted_path = secret_root
        .path()
        .join(format!("{}.v2", credential_key.as_str()));
    let encrypted_bytes = fs::read(&encrypted_path).expect("encrypted credential bytes");
    assert!(!String::from_utf8_lossy(&encrypted_bytes).contains(plaintext));
    let denied_decrypt = run_keychain_decrypt_probe(
        &second_build,
        keychain.path(),
        keychain.password(),
        super::ROUTER_KEYCHAIN_SERVICE,
        account,
        secret_root.path(),
        credential_key.as_str(),
    )
    .expect("second build should start");
    assert_eq!(
        denied_decrypt.status.code(),
        Some(41),
        "an unapproved build must receive a Keychain access denial with interaction disabled before it can decrypt; probe stderr: {}",
        String::from_utf8_lossy(&denied_decrypt.stderr)
    );
    assert!(
        run_keychain_acl_probe(
            &first_build,
            "read",
            keychain.path(),
            keychain.password(),
            super::ROUTER_KEYCHAIN_SERVICE,
            account,
            "",
        )
        .expect("first build should start")
        .success(),
        "the first build should read its key without another prompt"
    );
    let denied_read = run_keychain_acl_probe(
        &second_build,
        "read",
        keychain.path(),
        keychain.password(),
        super::ROUTER_KEYCHAIN_SERVICE,
        account,
        "",
    )
    .expect("second build should start");
    assert_eq!(
        denied_read.code(),
        Some(41),
        "an unapproved build must receive a Keychain access denial while interaction is disabled"
    );
    assert_eq!(
        encrypted_store
            .read_secret(&credential_key)
            .expect("authorized test store should decrypt its credential")
            .expose_secret(),
        plaintext
    );
}
