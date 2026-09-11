//! Unix backend for [`muffin_platform`].
//!
//! This is the only crate that talks to `nix`/`libc` directly. It implements
//! the [`Platform`] and [`ProcessModel`] traits for Linux, macOS and the BSDs.
//!
//! # TerminalState opaque buffer
//!
//! The platform crate's [`TerminalState`] is an opaque `[u8; 64]` buffer.
//! Unix `termios` fits comfortably inside it (60 bytes on Linux, 36 on macOS);
//! we cast the buffer to `libc::termios` for `tcgetattr`/`tcsetattr` and to
//! mutate flags for raw mode. A compile-time assertion below fails the build
//! on any platform whose `termios` (or `sigset_t`) grows past the opaque
//! buffer, instead of silently overflowing.

use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use muffin_platform::{
    ChildFd, Fd, FileInfo, FileOpenMode, Platform, PlatformError, ProcessError, ProcessGroupId,
    ProcessHandle, ProcessModel, Signal, SignalMask, SpawnConfig, TerminalSize, TerminalState,
    WaitOptions, WaitStatus, XdgKind,
};
use nix::unistd::{ForkResult, Pid};

/// Compile-time guard: the opaque [`TerminalState`] buffer holds 64 bytes.
const _: () = assert!(core::mem::size_of::<libc::termios>() <= 64);
/// Compile-time guard: the opaque [`SignalMask`] buffer holds 128 bytes.
const _: () = assert!(core::mem::size_of::<libc::sigset_t>() <= 128);

/// The Unix platform backend. Stateless: most operations are syscalls on
/// process-global state.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnixPlatform;

/// Construct a `'static` Unix backend for `muffin_platform::init`.
pub fn unix_backend() -> &'static dyn ProcessModel {
    static BACKEND: UnixPlatform = UnixPlatform;
    &BACKEND
}

/// Signals received since the last drain, one bit per raw signal number.
/// Set only from signal handlers (async context), so only atomic ops.
static RECEIVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Generic handler: record the signal number in [`RECEIVED`].
extern "C" fn record_signal(sig: i32) {
    if (0..64).contains(&sig) {
        RECEIVED.fetch_or(1u64 << sig, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Apply raw-mode flags to a `libc::termios` stored in the opaque buffer.
fn raw_mode_fn(data: &mut [u8; 64]) {
    let t = data.as_mut_ptr() as *mut libc::termios;
    unsafe {
        (*t).c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        (*t).c_oflag &= !libc::OPOST;
        (*t).c_cflag |= libc::CS8;
        (*t).c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        (*t).c_cc[libc::VMIN] = 1;
        (*t).c_cc[libc::VTIME] = 0;
    }
}

// ---------------------------------------------------------------------------
// Platform implementation
// ---------------------------------------------------------------------------

impl Platform for UnixPlatform {
    // --- Terminal ---

    fn read_terminal_state(&self, fd: Fd) -> Result<TerminalState, PlatformError> {
        let mut data = [0u8; 64];
        let t = data.as_mut_ptr() as *mut libc::termios;
        let ret = unsafe { libc::tcgetattr(fd, t) };
        if ret != 0 {
            return Err(PlatformError::Io("tcgetattr failed".into()));
        }
        Ok(TerminalState::new(data, raw_mode_fn))
    }

    fn write_terminal_state(&self, fd: Fd, state: &TerminalState) -> Result<(), PlatformError> {
        let t = state.data().as_ptr() as *const libc::termios;
        let ret = unsafe { libc::tcsetattr(fd, libc::TCSANOW, t) };
        if ret != 0 {
            return Err(PlatformError::Io("tcsetattr failed".into()));
        }
        Ok(())
    }

    fn terminal_size(&self, fd: Fd) -> Result<TerminalSize, PlatformError> {
        let mut ws: libc::winsize = unsafe { core::mem::zeroed() };
        let ret = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
        if ret != 0 {
            return Err(PlatformError::Io("TIOCGWINSZ failed".into()));
        }
        Ok(TerminalSize {
            rows: ws.ws_row,
            cols: ws.ws_col,
        })
    }

    // --- FD ---

    fn create_pipe_pair(&self, cloexec: bool) -> Result<(Fd, Fd), PlatformError> {
        use std::os::fd::IntoRawFd;
        let flags = if cloexec {
            nix::fcntl::OFlag::O_CLOEXEC
        } else {
            nix::fcntl::OFlag::empty()
        };
        nix::unistd::pipe2(flags)
            .map(|(r, w)| (r.into_raw_fd(), w.into_raw_fd()))
            .map_err(|e| PlatformError::Io(format!("pipe2 failed: {e}")))
    }

    fn open_file(&self, path: &str, mode: FileOpenMode) -> Result<Fd, PlatformError> {
        use std::fs::OpenOptions;
        use std::os::fd::IntoRawFd;
        let mut opts = OpenOptions::new();
        match mode {
            FileOpenMode::Read => {
                opts.read(true);
            }
            FileOpenMode::Write => {
                opts.write(true).create(true).truncate(true);
            }
            FileOpenMode::Append => {
                opts.write(true).create(true).append(true);
            }
            FileOpenMode::ReadWrite => {
                opts.read(true).write(true).create(true);
            }
            FileOpenMode::Clobber => {
                opts.write(true).create(true).truncate(true);
            }
        }
        let f = opts
            .open(path)
            .map_err(|e| PlatformError::Io(format!("open {path}: {e}")))?;
        Ok(f.into_raw_fd())
    }

    fn duplicate_fd(&self, fd: Fd) -> Result<Fd, PlatformError> {
        let ret = unsafe { libc::dup(fd) };
        if ret < 0 {
            Err(PlatformError::Io("dup failed".into()))
        } else {
            Ok(ret)
        }
    }

    fn duplicate_fd_to(&self, oldfd: Fd, newfd: Fd) -> Result<(), PlatformError> {
        if unsafe { libc::dup2(oldfd, newfd) } < 0 {
            Err(PlatformError::Io("dup2 failed".into()))
        } else {
            Ok(())
        }
    }

    fn close(&self, fd: Fd) -> Result<(), PlatformError> {
        if unsafe { libc::close(fd) } < 0 {
            Err(PlatformError::Io("close failed".into()))
        } else {
            Ok(())
        }
    }

    fn write(&self, fd: Fd, buf: &[u8]) -> Result<usize, PlatformError> {
        let ret = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
        if ret < 0 {
            Err(PlatformError::Io("write failed".into()))
        } else {
            Ok(ret as usize)
        }
    }

    fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, PlatformError> {
        let ret = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if ret < 0 {
            Err(PlatformError::Io("read failed".into()))
        } else {
            Ok(ret as usize)
        }
    }

    // --- FS ---

    fn null_device(&self) -> &'static str {
        "/dev/null"
    }

    fn path_separator(&self) -> char {
        '/'
    }

    fn is_path_separator(&self, c: char) -> bool {
        c == '/'
    }

    fn is_executable(&self, path: &str) -> bool {
        let p = std::path::Path::new(path);
        p.is_file() && {
            use std::os::unix::fs::PermissionsExt;
            p.metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
    }

    fn file_info(&self, path: &str) -> FileInfo {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
        let p = std::path::Path::new(path);
        let mut info = FileInfo {
            exists: p.exists(),
            ..Default::default()
        };
        // Use symlink_metadata so -h/-L work correctly on symlinks
        let md = match std::fs::symlink_metadata(path) {
            Ok(md) => md,
            Err(_) => return info,
        };
        let ft = md.file_type();
        info.is_file = ft.is_file();
        info.is_dir = ft.is_dir();
        info.is_symlink = ft.is_symlink();
        info.is_socket = ft.is_socket();
        info.is_block_device = ft.is_block_device();
        info.is_char_device = ft.is_char_device();
        info.is_fifo = ft.is_fifo();
        info.size = md.len();

        let mode = md.permissions().mode();
        info.is_readable = mode & 0o444 != 0;
        info.is_writable = mode & 0o222 != 0;
        info.is_executable = mode & 0o111 != 0;
        info.has_suid = mode & 0o4000 != 0;
        info.has_sgid = mode & 0o2000 != 0;
        info.has_sticky = mode & 0o1000 != 0;

        info.uid = md.uid();
        info.gid = md.gid();
        info.mtime = md.mtime();
        info.atime = md.atime();
        info.dev = md.dev();
        info.ino = md.ino();

        info
    }

    fn is_terminal_fd(&self, fd: u32) -> bool {
        unsafe { libc::isatty(fd as i32) != 0 }
    }

    fn read_dir(&self, path: &str) -> Result<Vec<String>, PlatformError> {
        let mut names: Vec<String> = std::fs::read_dir(path)
            .map_err(|e| PlatformError::Io(format!("read_dir {path}: {e}")))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(names)
    }

    fn xdg_dir(&self, kind: XdgKind) -> String {
        use std::env;
        let (env_name, fallback_sub) = match kind {
            XdgKind::Data => ("XDG_DATA_HOME", ".local/share"),
            XdgKind::Config => ("XDG_CONFIG_HOME", ".config"),
            XdgKind::Cache => ("XDG_CACHE_HOME", ".cache"),
        };
        if let Ok(dir) = env::var(env_name)
            && !dir.is_empty()
        {
            return dir;
        }
        let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home)
            .join(fallback_sub)
            .to_string_lossy()
            .into_owned()
    }

    // --- CWD ---

    fn current_dir(&self) -> String {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| String::new())
    }

    fn set_current_dir(&self, path: &str) -> Result<(), PlatformError> {
        std::env::set_current_dir(path).map_err(|e| PlatformError::Io(e.to_string()))
    }

    // --- Time ---

    fn time_seconds(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn time_nanos(&self) -> u64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let start = *START.get_or_init(std::time::Instant::now);
        start.elapsed().as_nanos() as u64
    }

    fn local_time_hms(&self) -> (u8, u8, u8) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as libc::time_t)
            .unwrap_or(0);
        // SAFETY: localtime_r is reentrant and writes into our zeroed tm.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe {
            libc::localtime_r(&now, &mut tm);
        }
        (tm.tm_hour as u8, tm.tm_min as u8, tm.tm_sec as u8)
    }
}

// ---------------------------------------------------------------------------
// ProcessModel implementation
// ---------------------------------------------------------------------------

impl ProcessModel for UnixPlatform {
    // --- Process ---

    fn spawn(&self, cfg: &SpawnConfig) -> Result<ProcessHandle, ProcessError> {
        let path = CString::new(cfg.path.as_bytes()).map_err(|_| ProcessError::ExecFailed)?;
        let argv: Vec<CString> = cfg
            .argv
            .iter()
            .map(|a| CString::new(a.as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(|_| ProcessError::ExecFailed)?;
        let env: Vec<CString> = cfg
            .env
            .iter()
            .map(|(k, v)| CString::new(format!("{k}={v}")))
            .collect::<Result<_, _>>()
            .map_err(|_| ProcessError::ExecFailed)?;

        match unsafe { nix::unistd::fork() } {
            Ok(ForkResult::Parent { child }) => Ok(ProcessHandle::new(child.as_raw())),
            Ok(ForkResult::Child) => {
                // 1. Process group.
                if let Some(pgid) = cfg.pgroup {
                    let _ = nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(pgid.raw()));
                }
                // 2. Redirections.
                for (target, target_fd) in [(&cfg.stdin, 0), (&cfg.stdout, 1), (&cfg.stderr, 2)] {
                    if dup_to_target(target, target_fd, self.null_device()).is_err() {
                        unsafe { libc::_exit(1) };
                    }
                }
                // 3. Exec. On failure, exit with 126/127.
                match nix::unistd::execve(&path, &argv, &env) {
                    Ok(never) => match never {},
                    Err(err) => {
                        let code = if err == nix::errno::Errno::ENOENT {
                            127
                        } else {
                            126
                        };
                        unsafe { libc::_exit(code) };
                    }
                }
            }
            Err(err) => Err(ProcessError::Other(format!("fork failed: {err}"))),
        }
    }

    fn wait(
        &self,
        handle: &ProcessHandle,
        opts: WaitOptions,
    ) -> Result<WaitStatus, ProcessError> {
        use nix::sys::wait::{WaitPidFlag, waitpid};
        let pid = Pid::from_raw(handle.pid());
        let mut flags = WaitPidFlag::empty();
        if opts.contains(WaitOptions::UNTRACED) {
            flags.insert(WaitPidFlag::WUNTRACED);
        }
        if opts.contains(WaitOptions::CONTINUED) {
            flags.insert(WaitPidFlag::WCONTINUED);
        }
        if opts.contains(WaitOptions::NOHANG) {
            flags.insert(WaitPidFlag::WNOHANG);
        }
        match waitpid(pid, Some(flags)) {
            Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => Ok(WaitStatus::Exited(code as u8)),
            Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => {
                Ok(WaitStatus::Signaled(self.signal_from_number(sig as i32)))
            }
            Ok(nix::sys::wait::WaitStatus::Stopped(_, sig)) => {
                Ok(WaitStatus::Stopped(self.signal_from_number(sig as i32)))
            }
            Ok(nix::sys::wait::WaitStatus::Continued(_)) => Ok(WaitStatus::Continued),
            Ok(nix::sys::wait::WaitStatus::StillAlive) => Ok(WaitStatus::StillAlive),
            Ok(_) => Ok(WaitStatus::Exited(127)),
            Err(err) => Err(ProcessError::Other(format!("waitpid failed: {err}"))),
        }
    }

    fn kill(&self, handle: &ProcessHandle, sig: Signal) -> Result<(), ProcessError> {
        let pid = Pid::from_raw(handle.pid());
        nix::sys::signal::kill(pid, nix_signal(sig))
            .map_err(|e| ProcessError::Other(format!("kill failed: {e}")))
    }

    fn set_foreground_process_group(
        &self,
        pgid: ProcessGroupId,
    ) -> Result<(), PlatformError> {
        use std::os::fd::BorrowedFd;
        // SAFETY: fd 0 (stdin) is held open for the shell's lifetime.
        let stdin = unsafe { BorrowedFd::borrow_raw(0) };
        nix::unistd::tcsetpgrp(stdin, Pid::from_raw(pgid.raw()))
            .map_err(|e| PlatformError::Io(format!("tcsetpgrp failed: {e}")))
    }

    fn current_process_group(&self) -> ProcessGroupId {
        ProcessGroupId::new(nix::unistd::getpgrp().as_raw())
    }

    // --- Signal ---

    fn install_signal_handler(
        &self,
        sig: Signal,
        handler: extern "C" fn(i32),
    ) -> Result<(), PlatformError> {
        use nix::sys::signal::{SigAction, SigHandler, SigSet};
        let sa = SigAction::new(
            SigHandler::Handler(handler),
            nix::sys::signal::SaFlags::SA_RESTART,
            SigSet::empty(),
        );
        let nsig = nix_signal(sig);
        unsafe { nix::sys::signal::sigaction(nsig, &sa) }
            .map_err(|e| PlatformError::Io(format!("sigaction failed: {e}")))?;
        Ok(())
    }

    fn signal_to_number(&self, sig: Signal) -> i32 {
        nix_signal(sig) as i32
    }

    fn signal_from_number(&self, n: i32) -> Signal {
        match nix::sys::signal::Signal::try_from(n) {
            Ok(s) => match s {
                nix::sys::signal::Signal::SIGHUP => Signal::Hangup,
                nix::sys::signal::Signal::SIGINT => Signal::Interrupt,
                nix::sys::signal::Signal::SIGQUIT => Signal::Quit,
                nix::sys::signal::Signal::SIGILL => Signal::Illegal,
                nix::sys::signal::Signal::SIGABRT => Signal::Abort,
                nix::sys::signal::Signal::SIGBUS => Signal::Bus,
                nix::sys::signal::Signal::SIGFPE => Signal::FloatingPoint,
                nix::sys::signal::Signal::SIGKILL => Signal::Kill,
                nix::sys::signal::Signal::SIGSEGV => Signal::Segmentation,
                nix::sys::signal::Signal::SIGPIPE => Signal::Pipe,
                nix::sys::signal::Signal::SIGALRM => Signal::Alarm,
                nix::sys::signal::Signal::SIGTERM => Signal::Terminate,
                nix::sys::signal::Signal::SIGCHLD => Signal::Child,
                nix::sys::signal::Signal::SIGCONT => Signal::Continue,
                nix::sys::signal::Signal::SIGSTOP => Signal::Stop,
                nix::sys::signal::Signal::SIGTSTP => Signal::Tstp,
                nix::sys::signal::Signal::SIGTTIN => Signal::Ttin,
                nix::sys::signal::Signal::SIGTTOU => Signal::Ttou,
                nix::sys::signal::Signal::SIGWINCH => Signal::WindowChange,
                nix::sys::signal::Signal::SIGUSR1 => Signal::User1,
                nix::sys::signal::Signal::SIGUSR2 => Signal::User2,
                _ => Signal::Other(n),
            },
            Err(_) => Signal::Other(n),
        }
    }

    fn signal_name(&self, n: i32) -> &'static str {
        match nix::sys::signal::Signal::try_from(n) {
            Ok(s) => s.as_str().strip_prefix("SIG").unwrap_or("???"),
            Err(_) => "???",
        }
    }

    fn install_trap_handler(&self, sig: Signal) -> Result<(), PlatformError> {
        self.install_signal_handler(sig, record_signal)
    }

    fn block_signals(&self, sigs: &[Signal]) -> Result<SignalMask, PlatformError> {
        let mut set: libc::sigset_t = unsafe { core::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut set);
        }
        for sig in sigs {
            unsafe {
                libc::sigaddset(&mut set, self.signal_to_number(*sig));
            }
        }
        let mut old: libc::sigset_t = unsafe { core::mem::zeroed() };
        let ret = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old) };
        if ret != 0 {
            return Err(PlatformError::Io("pthread_sigmask failed".into()));
        }
        let mut data = [0u8; 128];
        let size = core::mem::size_of::<libc::sigset_t>();
        unsafe {
            core::ptr::copy_nonoverlapping(
                &old as *const libc::sigset_t as *const u8,
                data.as_mut_ptr(),
                size,
            );
        }
        Ok(SignalMask::new(data))
    }

    fn unblock_signals(&self, mask: &SignalMask) -> Result<(), PlatformError> {
        let old = unsafe { &*(mask.data().as_ptr() as *const libc::sigset_t) };
        let ret = unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, old, core::ptr::null_mut()) };
        if ret != 0 {
            return Err(PlatformError::Io("pthread_sigmask failed".into()));
        }
        Ok(())
    }

    fn receive_pending_signals(&self) -> Vec<Signal> {
        use std::sync::atomic::Ordering;
        let mut out = Vec::new();
        let bits = RECEIVED.load(Ordering::Relaxed);
        if bits == 0 {
            return out;
        }
        // Atomically claim the bits so the same signal is not re-delivered.
        let claimed = RECEIVED.fetch_and(0, Ordering::AcqRel) & bits;
        let mut n = 0;
        while (1u64 << n) <= claimed {
            if claimed & (1u64 << n) != 0 {
                out.push(self.signal_from_number(n));
            }
            n += 1;
        }
        out
    }

    // --- Subprocess ---

    fn fork_and_run(&self, f: &mut dyn FnMut() -> i32) -> Result<ProcessHandle, ProcessError> {
        match unsafe { nix::unistd::fork() } {
            Ok(ForkResult::Parent { child }) => Ok(ProcessHandle::new(child.as_raw())),
            Ok(ForkResult::Child) => {
                let code = f();
                unsafe { libc::_exit(code) };
            }
            Err(err) => Err(ProcessError::Other(format!("fork failed: {err}"))),
        }
    }

    // --- Process info ---

    fn effective_user_id(&self) -> u32 {
        unsafe { libc::geteuid() }
    }

    fn effective_group_id(&self) -> u32 {
        unsafe { libc::getegid() }
    }

    fn parent_pid(&self) -> i32 {
        unsafe { libc::getppid() }
    }

    fn fd_to_path(&self, fd: Fd) -> String {
        format!("/dev/fd/{fd}")
    }
}

fn nix_signal(sig: Signal) -> nix::sys::signal::Signal {
    match sig {
        Signal::Hangup => nix::sys::signal::Signal::SIGHUP,
        Signal::Interrupt => nix::sys::signal::Signal::SIGINT,
        Signal::Quit => nix::sys::signal::Signal::SIGQUIT,
        Signal::Illegal => nix::sys::signal::Signal::SIGILL,
        Signal::Abort => nix::sys::signal::Signal::SIGABRT,
        Signal::Bus => nix::sys::signal::Signal::SIGBUS,
        Signal::FloatingPoint => nix::sys::signal::Signal::SIGFPE,
        Signal::Kill => nix::sys::signal::Signal::SIGKILL,
        Signal::Segmentation => nix::sys::signal::Signal::SIGSEGV,
        Signal::Pipe => nix::sys::signal::Signal::SIGPIPE,
        Signal::Alarm => nix::sys::signal::Signal::SIGALRM,
        Signal::Terminate => nix::sys::signal::Signal::SIGTERM,
        Signal::Child => nix::sys::signal::Signal::SIGCHLD,
        Signal::Continue => nix::sys::signal::Signal::SIGCONT,
        Signal::Stop => nix::sys::signal::Signal::SIGSTOP,
        Signal::Tstp => nix::sys::signal::Signal::SIGTSTP,
        Signal::Ttin => nix::sys::signal::Signal::SIGTTIN,
        Signal::Ttou => nix::sys::signal::Signal::SIGTTOU,
        Signal::WindowChange => nix::sys::signal::Signal::SIGWINCH,
        Signal::User1 => nix::sys::signal::Signal::SIGUSR1,
        Signal::User2 => nix::sys::signal::Signal::SIGUSR2,
        Signal::Other(n) => {
            nix::sys::signal::Signal::try_from(n).unwrap_or(nix::sys::signal::Signal::SIGTERM)
        }
    }
}

/// Apply one child-fd redirection in the child of a fork.
fn dup_to_target(target: &ChildFd, target_fd: Fd, devnull: &str) -> Result<(), ()> {
    match target {
        ChildFd::Inherit => Ok(()),
        ChildFd::DevNull => {
            let devnull = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(devnull)
                .map_err(|_| ())?;
            let fd = devnull.as_raw_fd();
            if unsafe { libc::dup2(fd, target_fd) } < 0 {
                return Err(());
            }
            Ok(())
        }
        ChildFd::Fd(fd) => {
            if unsafe { libc::dup2(*fd, target_fd) } < 0 {
                return Err(());
            }
            Ok(())
        }
        ChildFd::File(path) => {
            let read = target_fd == 0;
            let f = if read {
                std::fs::OpenOptions::new().read(true).open(path)
            } else {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(path)
            }
            .map_err(|_| ())?;
            let fd = f.as_raw_fd();
            if unsafe { libc::dup2(fd, target_fd) } < 0 {
                return Err(());
            }
            Ok(())
        }
        ChildFd::Close => {
            if unsafe { libc::close(target_fd) } < 0 {
                return Err(());
            }
            Ok(())
        }
    }
}
