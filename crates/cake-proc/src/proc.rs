use alloc::string::String;
use alloc::vec::Vec;

/// The exit status of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcStatus {
    /// The process exited with the given code.
    Exit(i32),
    /// The process was terminated by the given signal.
    Signal(i32),
    /// The process has not started (or was never a real process).
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

impl Default for ProcStatus {
    fn default() -> Self {
        ProcStatus::NotStarted
    }
}

/// How a process in a job is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessType {
    /// An external executable.
    External,
    /// A builtin command (runs in the shell process).
    Builtin,
    /// A shell function.
    Function,
    /// A compound command (if/for/while/... or a block).
    Compound,
}

/// One element of a pipeline.
#[derive(Debug, Clone)]
pub struct Process {
    /// The command word and its arguments (argv[0] first).
    pub argv: Vec<String>,
    pub typ: ProcessType,
    pub status: ProcStatus,
    /// True if this process is the first element of a pipeline.
    pub is_first_in_job: bool,
    /// True if this process is the last element of a pipeline.
    pub is_last_in_job: bool,
}

impl Process {
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            typ: ProcessType::External,
            status: ProcStatus::NotStarted,
            is_first_in_job: true,
            is_last_in_job: true,
        }
    }
}
