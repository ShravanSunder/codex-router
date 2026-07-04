/// Keyboard action understood by the sessions picker model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionsPickerKey {
    MoveDown,
    MoveUp,
    PageDown,
    PageUp,
    MoveFirst,
    MoveLast,
    CycleRoot,
    CycleSource,
    CycleSort,
    SearchChar(char),
    SearchBackspace,
    ClearSearch,
}

/// User action selected from the sessions picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SessionsPickerOutcome {
    ResumeSession(String),
    StartNewSession,
    TerminalTooNarrow,
}
