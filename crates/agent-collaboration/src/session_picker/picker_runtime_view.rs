//! Runtime status view selection; provider-family visibility is independent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SessionsPickerRuntimeView {
    Blocked,
    Active,
    Idle,
    #[default]
    All,
}

impl SessionsPickerRuntimeView {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::All => Self::Blocked,
            Self::Blocked => Self::Active,
            Self::Active => Self::Idle,
            Self::Idle => Self::All,
        }
    }
}
