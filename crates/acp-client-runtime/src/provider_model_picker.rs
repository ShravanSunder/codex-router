//! Explicit provider-specific model-picker negotiation, separate from permissions.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProviderModelPicker {
    #[default]
    Standard,
    CursorParameterized,
}
