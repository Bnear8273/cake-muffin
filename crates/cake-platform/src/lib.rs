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

/// A signal, by symbolic name (not by platform-specific number).
///
/// Shell code refers to signals symbolically; the platform backend owns the
/// mapping to raw numbers ([`ProcessModel::signal_to_number`]), because those
/// numbers differ between Linux and the BSDs/macOS (e.g. `SIGUSR1` is 10 on
/// Linux but 30 on macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// `SIGHUP` — hangup (controlling terminal closed).
    Hangup,
    /// `SIGINT` — interactive interrupt (Ctrl-C).
    Interrupt,
    /// `SIGQUIT` — keyboard quit (Ctrl-\).
    Quit,
    /// `SIGILL` — illegal instruction.
    Illegal,
    /// `SIGABRT` — abort.
    Abort,
    /// `SIGBUS` — bus error.
    Bus,
    /// `SIGFPE` — floating-point exception.
    FloatingPoint,
    /// `SIGKILL` — kill (cannot be caught or ignored).
    Kill,
    /// `SIGSEGV` — invalid memory reference.
    Segmentation,
    /// `SIGPIPE` — broken pipe.
    Pipe,
    /// `SIGALRM` — timer alarm.
    Alarm,
    /// `SIGTERM` — termination.
    Terminate,
    /// `SIGCHLD` — child stopped or terminated.
    Child,
    /// `SIGCONT` — continue a stopped process.
    Continue,
    /// `SIGSTOP` — stop (cannot be caught or ignored).
    Stop,
    /// `SIGTSTP` — keyboard stop (Ctrl-Z).
    Tstp,
    /// `SIGTTIN` — background read from terminal.
    Ttin,
    /// `SIGTTOU` — background write to terminal.
    Ttou,
    /// `SIGWINCH` — window size change.
    WindowChange,
    /// `SIGUSR1` — application-defined.
    User1,
    /// `SIGUSR2` — application-defined.
    User2,
    /// Any other signal by its raw number.
    Other(i32),
}

impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Signal::Hangup => "SIGHUP",
            Signal::Interrupt => "SIGINT",
            Signal::Quit => "SIGQUIT",
            Signal::Illegal => "SIGILL",
            Signal::Abort => "SIGABRT",
            Signal::Bus => "SIGBUS",
            Signal::FloatingPoint => "SIGFPE",
            Signal::Kill => "SIGKILL",
            Signal::Segmentation => "SIGSEGV",
            Signal::Pipe => "SIGPIPE",
            Signal::Alarm => "SIGALRM",
            Signal::Terminate => "SIGTERM",
            Signal::Child => "SIGCHLD",
            Signal::Continue => "SIGCONT",
            Signal::Stop => "SIGSTOP",
            Signal::Tstp => "SIGTSTP",
            Signal::Ttin => "SIGTTIN",
            Signal::Ttou => "SIGTTOU",
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
    /// The child has not exited yet (only with `NOHANG`).
    StillAlive,
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

/// Opaque terminal state snapshot. Can be saved and restored to enter/exit
/// raw mode for line editing.
pub struct TerminalState {
    data: [u8; 64],
    raw_mode_fn: fn(&mut [u8; 64]),
}

impl TerminalState {
    pub fn new(data: [u8; 64], raw_mode_fn: fn(&mut [u8; 64])) -> Self {
        Self {
            data,
            raw_mode_fn,
        }
    }

    pub fn data(&self) -> &[u8; 64] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8; 64] {
        &mut self.data
    }

    /// Apply raw mode flags (no echo, no canonical, etc.) to the stored state.
    pub fn set_raw(&mut self) {
        (self.raw_mode_fn)(&mut self.data);
    }
}

impl fmt::Debug for TerminalState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalState")
            .field("data", &"[...]")
            .finish_non_exhaustive()
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
    // File type flags
    pub is_socket: bool,
    pub is_block_device: bool,
    pub is_char_device: bool,
    pub is_fifo: bool,
    // Permission bits
    pub has_suid: bool,
    pub has_sgid: bool,
    pub has_sticky: bool,
    // Ownership
    pub uid: u32,
    pub gid: u32,
    // Timestamps (seconds since epoch)
    pub mtime: i64,
    pub atime: i64,
    // Identity (for -ef)
    pub dev: u64,
    pub ino: u64,
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

/// Lowest-level OS abstraction: files, descriptors, I/O, terminal, CWD, time.
/// Owns no process semantics — see [`ProcessModel`].
///
/// Backends implement this trait; the shell drives it through
/// [`ProcessModel`] (which inherits `Platform`).
pub trait Platform: Sync {
    // --- Terminal ---

    /// Read the full terminal state (attributes, flags, control characters)
    /// for saving/restoring around raw mode.
    fn read_terminal_state(&self, _fd: Fd) -> Result<TerminalState, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Write a previously saved terminal state back.
    fn write_terminal_state(
        &self,
        _fd: Fd,
        _state: &TerminalState,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Terminal window size in rows and columns.
    fn terminal_size(&self, _fd: Fd) -> Result<TerminalSize, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    // --- FD ---

    /// Create a unidirectional pipe, returning (read_fd, write_fd).
    fn create_pipe_pair(&self, cloexec: bool) -> Result<(Fd, Fd), PlatformError>;

    /// Open a file by path with the given mode, returning its fd.
    fn open_file(&self, path: &str, mode: FileOpenMode) -> Result<Fd, PlatformError>;

    /// Duplicate `fd` to the lowest available number.
    fn duplicate_fd(&self, fd: Fd) -> Result<Fd, PlatformError>;

    /// Duplicate `oldfd` to `newfd`, closing `newfd` first if open.
    fn duplicate_fd_to(&self, oldfd: Fd, newfd: Fd) -> Result<(), PlatformError>;

    /// Close a file descriptor.
    fn close(&self, fd: Fd) -> Result<(), PlatformError>;

    /// Write up to `buf.len()` bytes; returns bytes written.
    fn write(&self, fd: Fd, buf: &[u8]) -> Result<usize, PlatformError>;

    /// Read into `buf`; returns bytes read (0 at EOF).
    fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, PlatformError>;

    // --- FS ---

    /// The platform's null device path (`/dev/null` on Unix, `NUL` on
    /// Windows).
    fn null_device(&self) -> &'static str;

    /// The platform's preferred path separator (`/` on Unix, `\` on Windows).
    fn path_separator(&self) -> char;

    /// Whether `c` is accepted as a path separator. Each platform must
    /// define this explicitly — there is no universal default.
    fn is_path_separator(&self, c: char) -> bool;

    /// Whether `path` points to an executable file.
    fn is_executable(&self, path: &str) -> bool;

    /// Filesystem facts for test operators (`[[ -e ]]`, `[[ -d ]]`, etc.).
    fn file_info(&self, path: &str) -> FileInfo;

    /// True if `fd` refers to a terminal (isatty).
    fn is_terminal_fd(&self, fd: u32) -> bool;

    /// List the entry names in `path` (glob expansion support).
    fn read_dir(&self, path: &str) -> Result<Vec<String>, PlatformError>;

    /// XDG base directory path.
    fn xdg_dir(&self, kind: XdgKind) -> String;

    // --- CWD ---

    /// Current working directory.
    fn current_dir(&self) -> String;

    /// Set the current working directory.
    fn set_current_dir(&self, path: &str) -> Result<(), PlatformError>;

    // --- Time ---

    /// Wall-clock seconds since the Unix epoch (for `$SECONDS`/`$RANDOM`).
    fn time_seconds(&self) -> i64;

    /// Monotonic nanoseconds since an arbitrary origin (for elapsed-time
    /// measurements like the prompt's command-execution-time segment).
    fn time_nanos(&self) -> u64;

    /// Local time components (hours, minutes, seconds) for the clock segment.
    fn local_time_hms(&self) -> (u8, u8, u8);
}

// ---------------------------------------------------------------------------
// ProcessModel trait
// ---------------------------------------------------------------------------

/// Process and signal management. Inherits [`Platform`] so a single
/// `&dyn ProcessModel` reference gives access to both OS primitives and
/// process semantics.
///
/// A target without Unix primitives (e.g. no `fork`) implements
/// `ProcessModel` with default stubs for unsupported operations
/// (`Err(Unsupported)`).
pub trait ProcessModel: Platform {
    // --- Process ---

    /// Spawn a child process (fork+exec on Unix, CreateProcess on Windows).
    fn spawn(&self, cfg: &SpawnConfig) -> Result<ProcessHandle, ProcessError>;

    /// Wait for a child process to change status.
    fn wait(
        &self,
        handle: &ProcessHandle,
        opts: WaitOptions,
    ) -> Result<WaitStatus, ProcessError>;

    /// Send a signal to a process.
    fn kill(&self, handle: &ProcessHandle, sig: Signal) -> Result<(), ProcessError>;

    /// Give terminal foreground to a process group. Returns
    /// `Err(Unsupported)` on systems without job control.
    fn set_foreground_process_group(
        &self,
        _pgid: ProcessGroupId,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Current process group ID.
    fn current_process_group(&self) -> ProcessGroupId;

    // --- Signal ---

    /// Install a C signal handler. Returns `Err(Unsupported)` on systems
    /// without signals.
    fn install_signal_handler(
        &self,
        _sig: Signal,
        _handler: extern "C" fn(i32),
    ) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Map a symbolic signal to the platform's raw signal number.
    ///
    /// Numbers are platform-specific (e.g. `SIGUSR1` is 10 on Linux but 30
    /// on macOS/BSD), so the backend owns the mapping.
    fn signal_to_number(&self, sig: Signal) -> i32;

    /// Map a raw signal number back to a symbolic signal; unknown numbers
    /// become [`Signal::Other`].
    fn signal_from_number(&self, n: i32) -> Signal;

    /// The short name of a raw signal number, e.g. `"INT"` for SIGINT, or
    /// `"???"` for unknown numbers.
    fn signal_name(&self, n: i32) -> &'static str;

    /// Install the shell's recording handler for `sig` so that `trap` can
    /// react to it at the next evaluation boundary. Returns
    /// `Err(Unsupported)` on systems without signals.
    fn install_trap_handler(&self, _sig: Signal) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Block the given signals, returning the previous mask so callers can
    /// restore it with `unblock_signals`. Returns `Err(Unsupported)` on
    /// systems without signals.
    fn block_signals(&self, _sigs: &[Signal]) -> Result<SignalMask, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Restore a previously returned signal mask. Returns
    /// `Err(Unsupported)` on systems without signals.
    fn unblock_signals(&self, _mask: &SignalMask) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    /// Drain the signals received since the last call (used for `trap`).
    /// Returns an empty vec on systems without signals.
    fn receive_pending_signals(&self) -> Vec<Signal> {
        Vec::new()
    }

    // --- Subprocess ---

    /// Run `f` in a child process. The child calls `_exit(f())`.
    ///
    /// # Portability
    ///
    /// The Unix backend implements this with `fork()`, which snapshots the
    /// whole process (heap, open fds, signals) and lets the child continue
    /// running arbitrary shell code in-place. Windows has no `fork`; a future
    /// Windows backend must implement this differently (e.g. spawn a fresh
    /// `cake -c <script>` process and wire up stdin/stdout/stderr), so callers
    /// should keep the closure free of non-serializable state.
    ///
    /// Returns `Err(Unsupported)` on systems without `fork`.
    fn fork_and_run(
        &self,
        _f: &mut dyn FnMut() -> i32,
    ) -> Result<ProcessHandle, ProcessError> {
        Err(ProcessError::Other(
            "fork_and_run not supported on this platform".into(),
        ))
    }

    // --- Process info ---

    /// Effective user ID (0 = root on Unix).
    fn effective_user_id(&self) -> u32 {
        0
    }

    /// Effective group ID (0 = root on Unix).
    fn effective_group_id(&self) -> u32 {
        0
    }

    /// Parent process ID (for `$PPID`).
    fn parent_pid(&self) -> i32;

    /// The path under which `fd` can be opened (`/dev/fd/N` on Unix).
    fn fd_to_path(&self, fd: Fd) -> String;
}

// ---------------------------------------------------------------------------
// Global accessor
// ---------------------------------------------------------------------------

static PLATFORM: spin::Once<&'static dyn ProcessModel> = spin::Once::new();

/// Initialize the global platform instance. Must be called once before any
/// call to [`get`], and only from a single-threaded context.
///
/// # Panics
///
/// Panics if called more than once.
pub fn init(p: &'static dyn ProcessModel) {
    PLATFORM.call_once(|| p);
}

/// Access the global platform instance.
///
/// # Panics
///
/// Panics if [`init`] has not been called yet.
pub fn get() -> &'static dyn ProcessModel {
    *PLATFORM
        .get()
        .expect("cake_platform::init has not been called")
}


