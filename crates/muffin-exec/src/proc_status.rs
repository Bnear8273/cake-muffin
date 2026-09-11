//! Process exit status.
//!
//! The shell-level interpretation of a child's [`WaitStatus`](muffin_platform::WaitStatus):
//! exit codes pass through, signal deaths report `128 + signal`, plus two
//! shell-internal states (`NotStarted`, `Cancelled`) for jobs that never
//! ran or were interrupted.

/// The exit status of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcStatus {
    /// The process exited with the given code.
    Exit(i32),
    /// The process was terminated by the given signal.
    Signal(i32),
    /// The process has not started (or was never a real process).
    #[default]
    NotStarted,
    /// The job was cancelled (e.g. by SIGINT).
    Cancelled,
}

impl ProcStatus {
    pub fn from_exit_code(code: i32) -> Self {
        ProcStatus::Exit(code)
    }

    pub fn from_signal(sig: i32) -> Self {
        ProcStatus::Signal(sig)
    }

    /// Whether the process succeeded (exit 0).
    pub fn success(&self) -> bool {
        matches!(self, ProcStatus::Exit(0))
    }

    /// Whether the process has produced a final status.
    pub fn is_done(&self) -> bool {
        !matches!(self, ProcStatus::NotStarted)
    }

    /// The numeric status as observed by `$?`.
    ///
    /// Follows bash: exit codes pass through, signal-terminated processes
    /// report `128 + signal`.
    pub fn status_code(&self) -> i32 {
        match self {
            ProcStatus::Exit(code) => *code,
            ProcStatus::Signal(sig) => 128 + sig,
            ProcStatus::NotStarted => 0,
            ProcStatus::Cancelled => 130, // 128 + SIGINT
        }
    }
}
