#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LinkIdentityError {
    #[error("ProviderLink identity must be a valid UUID")]
    MalformedUuid,
    #[error("ProviderLink identity must not be nil")]
    NilUuid,
}
