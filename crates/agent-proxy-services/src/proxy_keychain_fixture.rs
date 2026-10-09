//! External Keychain stand-in: in-memory deterministic bytes, never a platform read.
use codex_router_secret_store::keychain_data_key::{
    KeychainAccess, KeychainAccessError, ROUTER_KEYCHAIN_SERVICE,
};
pub(crate) struct ProxyFixtureKeychain;
impl KeychainAccess for ProxyFixtureKeychain {
    fn read_secret(
        &self,
        service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        Ok(Some(vec![0x54; 32]))
    }
    fn read_secret_without_user_interaction(
        &self,
        service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        Ok(Some(vec![0x54; 32]))
    }
    fn add_secret(
        &self,
        _service: &str,
        _account: &str,
        _secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        Err(KeychainAccessError::TestKeychainAccessForbidden)
    }
}
