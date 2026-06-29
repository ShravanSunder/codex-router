/// Keyboard action understood by the sessions picker model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionsPickerKey {
    MoveDown,
    MoveUp,
    CycleRoot,
    CycleProvider,
    CycleSource,
    SearchChar(char),
    SearchBackspace,
}

/// User action selected from the sessions picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SessionsPickerOutcome {
    ResumeSession(String),
    StartNewSession,
    TerminalTooNarrow,
}
