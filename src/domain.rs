/// The high-level result of one application invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The user canceled before any filesystem mutation was possible.
    Cancelled,
    /// The CLI foundation completed its startup setup.
    StartupConfigured,
}

impl RunOutcome {
    /// Returns the process exit code for this non-mutating outcome.
    pub const fn exit_code(self) -> i32 {
        0
    }
}
