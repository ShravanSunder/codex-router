use super::*;

pub(super) static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) fn account_id(value: &str) -> AccountId {
    AccountId::new(value).unwrap_or_else(|error| panic!("test account id should parse: {error}"))
}
