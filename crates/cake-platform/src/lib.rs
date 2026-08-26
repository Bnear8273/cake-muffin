#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// File descriptor. On Unix this is `i32`; on Windows `usize`.
#[cfg(unix)]
pub type Fd = i32;
#[cfg(not(unix))]
pub type Fd = usize;

/// A signal number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Interrupt,
    Quit,
    Terminate,
    Child,
    Continue,
    Stop,
    WindowChange,
    User1,
    User2,
    /// Any other signal by its raw number.
    Other(i32),
}

impl Signal {
    pub fn number(self) -> i32 {
        match self {
            Signal::Interrupt => 2,
            Signal::Quit => 3,
            Signal::Terminate => 15,
            Signal::Child => 17,
            Signal::Continue => 18,
            Signal::Stop => 19,
            Signal::WindowChange => 28,
            Signal::User1 => 10,
            Signal::User2 => 12,
            Signal::Other(n) => n,
        }
    }

    pub fn from_number(n: i32) -> Self {
        match n {
            2 => Signal::Interrupt,
            3 => Signal::Quit,
            10 => Signal::User1,
            12 => Signal::User2,
            15 => Signal::Terminate,
            17 => Signal::Child,
            18 => Signal::Continue,
            19 => Signal::Stop,
            28 => Signal::WindowChange,
            _ => Signal::Other(n),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Signal::Interrupt => "SIGINT",
            Signal::Quit => "SIGQUIT",
            Signal::Terminate => "SIGTERM",
            Signal::Child => "SIGCHLD",
            Signal::Continue => "SIGCONT",
            Signal::Stop => "SIGTSTP",
            Signal::WindowChange => "SIGWINCH",
            Signal::User1 => "SIGUSR1",
            Signal::User2 => "SIGUSR2",
            Signal::Other(_) => "SIG???",
        }
    }
}

/// The exit status of a child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitStatus {
    Exited(u8),
    Signaled(Signal),
    Stopped(Signal),
    Continued,
}

/// Opaque signal mask, for `block_signals`/`unblock_signals`.
pub struct SignalMask {
    data: [u8; 128],
}

impl SignalMask {
    pub fn new(data: [u8; 128]) -> Self {
        Self { data }
    }

    pub fn data(&self) -> &[u8; 128] {
        &self.data
    }
}

impl fmt::Debug for SignalMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalMask").finish_non_exhaustive()
    }
}

/// Options for `wait`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaitOptions(u8);

impl WaitOptions {
    pub const NONE: WaitOptions = WaitOptions(0);
    pub const UNTRACED: WaitOptions = WaitOptions(1);
    pub const CONTINUED: WaitOptions = WaitOptions(2);
    pub const NOHANG: WaitOptions = WaitOptions(4);

    pub fn contains(self, other: WaitOptions) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Terminal size in rows and columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

/// Opaque terminal attributes.
pub struct Termios {
    data: [u8; 64],
    raw_fn: fn(&mut [u8; 64]),
}

impl Termios {
    pub fn new(data: [u8; 64], raw_fn: fn(&mut [u8; 64])) -> Self {
        Self { data, raw_fn }
    }

    pub fn data(&self) -> &[u8; 64] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8; 64] {
        &mut self.data
    }

    /// Set the terminal to raw mode (no echo, no canonical, etc.).
    pub fn set_raw(&mut self) {
        (self.raw_fn)(&mut self.data);
    }
}

impl fmt::Debug for Termios {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Termios").finish_non_exhaustive()
    }
}

/// A handle to a child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessHandle {
    pid: i32,
}

impl ProcessHandle {
    pub fn new(pid: i32) -> Self {
        Self { pid }
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }
}

/// A process group identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessGroupId {
    pgid: i32,
}

impl ProcessGroupId {
    pub fn new(pgid: i32) -> Self {
        Self { pgid }
    }

    pub fn raw(&self) -> i32 {
        self.pgid
    }
}

/// How to set up a child's stdin/stdout/stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildFd {
    /// Inherit the parent's fd.
    Inherit,
    /// Connect to `/dev/null`.
    DevNull,
    /// Use a pre-opened fd (from `pipe`).
    Fd(Fd),
    /// Open a file by path.
    File(String),
    /// Close the fd in the child (`>&-` / `<&-`).
    Close,
}

/// How to open a file for redirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpenMode {
    /// `<` — read only.
    Read,
    /// `>` — write, truncate.
    Write,
    /// `>>` — write, append.
    Append,
    /// `<>` — read+write, create.
    ReadWrite,
    /// `>|` — write, truncate, clobber.
    Clobber,
}

/// Filesystem facts for `[[ -e ]]`, `[[ -d ]]`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileInfo {
    pub exists: bool,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_readable: bool,
    pub is_writable: bool,
    pub is_executable: bool,
    pub size: u64,
}

/// Configuration for spawning a child process.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// The executable to exec (already resolved to an absolute path by the
    /// caller's command resolution).
    pub path: String,
    /// argv passed to the child; `argv[0]` is the command name as displayed,
    /// which may differ from `path`.
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub stdin: ChildFd,
    pub stdout: ChildFd,
    pub stderr: ChildFd,
    /// Join this process group, or create a new one when `None`.
    pub pgroup: Option<ProcessGroupId>,
    pub background: bool,
}

/// XDG base directory kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdgKind {
    Data,
    Config,
    Cache,
}

/// Errors from process operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    ExecFailed,
    NotFound,
    PermissionDenied,
    Other(String),
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessError::ExecFailed => f.write_str("exec failed"),
            ProcessError::NotFound => f.write_str("not found"),
            ProcessError::PermissionDenied => f.write_str("permission denied"),
            ProcessError::Other(msg) => f.write_str(msg),
        }
    }
}

/// Errors from platform operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    Io(String),
    Unsupported,
    Other(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::Io(msg) => write!(f, "I/O error: {msg}"),
            PlatformError::Unsupported => f.write_str("unsupported operation"),
            PlatformError::Other(msg) => f.write_str(msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Platform trait
// ---------------------------------------------------------------------------

/// Platform abstraction. Backends implement this trait; the shell drives it
/// through the global accessor (`cake_platform::init` / `cake_platform::get`).
pub trait Platform: Sync {
    // --- Process ---
    fn spawn(&self, cfg: &SpawnConfig) -> Result<ProcessHandle, ProcessError>;
    fn wait(&self, handle: &ProcessHandle, opts: WaitOptions) -> Result<WaitStatus, ProcessError>;
    fn kill(&self, handle: &ProcessHandle, sig: Signal) -> Result<(), ProcessError>;
    fn set_terminal_foreground(&self, pgid: ProcessGroupId) -> Result<(), PlatformError>;
    fn current_process_group(&self) -> ProcessGroupId;

    // --- Signal ---
    fn install_signal_handler(
        &self,
        sig: Signal,
        handler: extern "C" fn(i32),
    ) -> Result<(), PlatformError>;
    /// Block the given signals, returning the previous mask so callers can
    /// restore it with `unblock_signals`.
    fn block_signals(&self, sigs: &[Signal]) -> Result<SignalMask, PlatformError>;
    /// Restore a previously returned signal mask.
    fn unblock_signals(&self, mask: &SignalMask) -> Result<(), PlatformError>;

    // --- Terminal ---
    fn get_termios(&self, fd: Fd) -> Result<Termios, PlatformError>;
    fn set_termios(&self, fd: Fd, t: &Termios) -> Result<(), PlatformError>;
    fn terminal_size(&self, fd: Fd) -> Result<TerminalSize, PlatformError>;

    // --- FD ---
    fn pipe(&self, cloexec: bool) -> Result<(Fd, Fd), PlatformError>;
    fn open_file(&self, path: &str, mode: FileOpenMode) -> Result<Fd, PlatformError>;
    fn dup(&self, fd: Fd) -> Result<Fd, PlatformError>;
    fn dup2(&self, oldfd: Fd, newfd: Fd) -> Result<(), PlatformError>;
    fn close(&self, fd: Fd) -> Result<(), PlatformError>;
    /// Write up to `buf.len()` bytes; returns bytes written.
    fn write(&self, fd: Fd, buf: &[u8]) -> Result<usize, PlatformError>;
    /// Read into `buf`; returns bytes read (0 at EOF).
    fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, PlatformError>;

    // --- Subprocess ---
    /// Fork and run `f` in the child process. The child calls `_exit(f())`.
    fn run_in_child(&self, f: &mut dyn FnMut() -> i32) -> Result<ProcessHandle, ProcessError>;

    // --- FS ---
    fn is_executable(&self, path: &str) -> bool;
    /// Filesystem facts for test operators.
    fn stat(&self, path: &str) -> FileInfo;
    fn xdg_dir(&self, kind: XdgKind) -> String;

    // --- CWD ---
    fn current_dir(&self) -> String;
    fn set_current_dir(&self, path: &str) -> Result<(), PlatformError>;
}

// ---------------------------------------------------------------------------
// Global accessor
// ---------------------------------------------------------------------------

static PLATFORM: spin::Once<&'static dyn Platform> = spin::Once::new();

/// Initialize the global platform instance. Must be called once before any
/// call to [`get`], and only from a single-threaded context.
///
/// # Panics
///
/// Panics if called more than once.
pub fn init(p: &'static dyn Platform) {
    PLATFORM.call_once(|| p);
}

/// Access the global platform instance.
///
/// # Panics
///
/// Panics if [`init`] has not been called yet.
pub fn get() -> &'static dyn Platform {
    *PLATFORM
        .get()
        .expect("cake_platform::init has not been called")
}

/// Run `f` with the given signals blocked, restoring the previous mask
/// afterwards. A convenience built on `block_signals`/`unblock_signals`.
///
/// # Panics
///
/// Panics if the platform is uninitialized or a mask operation fails.
pub fn with_signals_blocked<F: FnOnce() -> T, T>(sigs: &[Signal], f: F) -> T {
    let mask = get().block_signals(sigs).expect("block_signals failed");
    let result = f();
    get().unblock_signals(&mask).expect("unblock_signals failed");
    result
}