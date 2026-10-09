use super::KeychainAccessError;
use super::ensure_router_keychain_service;

pub(super) fn read_secret_without_user_interaction(
    service: &str,
    account: &str,
) -> Result<Option<Vec<u8>>, KeychainAccessError> {
    ensure_router_keychain_service(service)?;

    use apple_native_keyring_store::keychain::decode_error;
    use security_framework::item::ItemClass;
    use security_framework::item::ItemSearchOptions;
    use security_framework::item::SearchResult;
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::os::macos::keychain::SecPreferencesDomain;

    let user_keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)
        .map_err(|_| KeychainAccessError::Unavailable)?;
    let mut search = ItemSearchOptions::new();
    search
        .keychains(&[user_keychain])
        .class(ItemClass::generic_password())
        .service(service)
        .account(account)
        .load_data(true)
        .skip_authenticated_items(true);

    let results = match search.search() {
        Ok(results) => results,
        Err(error) => match decode_error(error) {
            keyring_core::Error::NoEntry => return Ok(None),
            _ => return Err(KeychainAccessError::Unavailable),
        },
    };

    match results.into_iter().next() {
        Some(SearchResult::Data(secret)) => Ok(Some(secret)),
        Some(_) => Err(KeychainAccessError::Unavailable),
        None => Ok(None),
    }
}
