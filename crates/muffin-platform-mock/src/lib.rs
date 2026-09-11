//! Mock backend for testing. Records spawned commands; returns scripted
//! statuses. No real OS calls.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::vec::Vec;

use muffin_platform::{
    Fd, FileInfo, FileOpenMode, Platform, PlatformError, ProcessError, ProcessGroupId,
    ProcessHandle, ProcessModel, Signal, SpawnConfig, WaitOptions, WaitStatus, XdgKind,
};

/// A mock platform that records all spawned commands and returns
/// pre-configured wait statuses.
pub struct MockPlatform {
    spawns: Mutex<Vec<SpawnConfig>>,
    wait_queue: Mutex<VecDeque<WaitStatus>>,
    cwd: Mutex<String>,
    env: Mutex<Vec<(String, String)>>,
    xdg: Mutex<String>,
    pending_signals: Mutex<Vec<Signal>>,
    files: Mutex<HashMap<String, FileInfo>>,
    dirs: Mutex<HashMap<String, Vec<String>>>,
    writes: Mutex<Vec<(Fd, Vec<u8>)>>,
}

impl Default for MockPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPlatform {
    pub fn new() -> Self {
        Self {
            spawns: Mutex::new(Vec::new()),
            wait_queue: Mutex::new(VecDeque::new()),
            cwd: Mutex::new("/tmp".into()),
            env: Mutex::new(Vec::new()),
            xdg: Mutex::new("/tmp/.local/share".into()),
            pending_signals: Mutex::new(Vec::new()),
            files: Mutex::new(HashMap::new()),
            dirs: Mutex::new(HashMap::new()),
            writes: Mutex::new(Vec::new()),
        }
    }

    /// Simulate a signal arriving (e.g. `kill -TERM $$` inside a test).
    pub fn deliver_signal(&self, sig: Signal) {
        self.pending_signals.lock().unwrap().push(sig);
    }

    fn spawns(&self) -> MutexGuard<'_, Vec<SpawnConfig>> {
        self.spawns.lock().unwrap()
    }

    fn wait_queue(&self) -> MutexGuard<'_, VecDeque<WaitStatus>> {
        self.wait_queue.lock().unwrap()
    }

    pub fn push_wait_status(&self, s: WaitStatus) {
        self.wait_queue().push_back(s);
    }

    pub fn recorded_spawns(&self) -> Vec<SpawnConfig> {
        self.spawns().clone()
    }

    pub fn set_cwd(&self, s: &str) {
        *self.cwd.lock().unwrap() = s.to_owned();
    }

    pub fn set_env(&self, k: &str, v: &str) {
        let mut env = self.env.lock().unwrap();
        env.retain(|(ek, _)| ek != k);
        env.push((k.to_owned(), v.to_owned()));
    }

    pub fn set_xdg(&self, s: &str) {
        *self.xdg.lock().unwrap() = s.to_owned();
    }

    /// Configure the [`FileInfo`] returned for `path` by [`Platform::file_info`].
    /// Unconfigured paths return [`FileInfo::default`].
    pub fn set_file_info(&self, path: &str, info: FileInfo) {
        self.files.lock().unwrap().insert(path.to_owned(), info);
    }

    /// Configure the entry names returned for `path` by [`Platform::read_dir`].
    /// Unconfigured paths return [`PlatformError::Unsupported`].
    pub fn set_dir_entries(&self, path: &str, entries: &[&str]) {
        self.dirs.lock().unwrap().insert(
            path.to_owned(),
            entries.iter().map(|s| s.to_string()).collect(),
        );
    }

    /// Drain all recorded `(fd, bytes)` writes since the last call.
    pub fn take_writes(&self) -> Vec<(Fd, Vec<u8>)> {
        std::mem::take(&mut *self.writes.lock().unwrap())
    }

    /// All bytes written to `fd` since the last [`MockPlatform::take_writes`],
    /// concatenated in order.
    pub fn written_to(&self, fd: Fd) -> Vec<u8> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(f, _)| *f == fd)
            .flat_map(|(_, b)| b.iter().copied())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Platform implementation
// ---------------------------------------------------------------------------

impl Platform for MockPlatform {
    fn open_file(&self, _path: &str, _mode: FileOpenMode) -> Result<Fd, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn create_pipe_pair(&self, _cloexec: bool) -> Result<(Fd, Fd), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn duplicate_fd(&self, _fd: Fd) -> Result<Fd, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn duplicate_fd_to(&self, _oldfd: Fd, _newfd: Fd) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn close(&self, _fd: Fd) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn write(&self, fd: Fd, buf: &[u8]) -> Result<usize, PlatformError> {
        // Record the bytes so tests can assert on builtin output.
        self.writes.lock().unwrap().push((fd, buf.to_vec()));
        Ok(buf.len())
    }

    fn read(&self, _fd: Fd, _buf: &mut [u8]) -> Result<usize, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn null_device(&self) -> &'static str {
        "/dev/null"
    }

    fn path_separator(&self) -> char {
        '/'
    }

    fn is_path_separator(&self, c: char) -> bool {
        c == '/'
    }

    fn is_executable(&self, _path: &str) -> bool {
        true
    }

    fn file_info(&self, path: &str) -> FileInfo {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .copied()
            .unwrap_or_default()
    }

    fn is_terminal_fd(&self, _fd: u32) -> bool {
        false
    }

    fn read_dir(&self, path: &str) -> Result<Vec<String>, PlatformError> {
        self.dirs
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or(PlatformError::Unsupported)
    }

    fn xdg_dir(&self, _kind: XdgKind) -> String {
        self.xdg.lock().unwrap().clone()
    }

    fn current_dir(&self) -> String {
        self.cwd.lock().unwrap().clone()
    }

    fn set_current_dir(&self, path: &str) -> Result<(), PlatformError> {
        *self.cwd.lock().unwrap() = path.to_owned();
        Ok(())
    }

    fn time_seconds(&self) -> i64 {
        1_000_000
    }

    fn time_nanos(&self) -> u64 {
        1_000_000_000
    }

    fn local_time_hms(&self) -> (u8, u8, u8) {
        (17, 51, 22)
    }
}

// ---------------------------------------------------------------------------
// ProcessModel implementation
// ---------------------------------------------------------------------------

impl ProcessModel for MockPlatform {
    fn spawn(&self, cfg: &SpawnConfig) -> Result<ProcessHandle, ProcessError> {
        self.spawns().push(cfg.clone());
        // Return a fake pid based on spawn count.
        Ok(ProcessHandle::new(self.spawns().len() as i32))
    }

    fn wait(
        &self,
        _handle: &ProcessHandle,
        _opts: WaitOptions,
    ) -> Result<WaitStatus, ProcessError> {
        self.wait_queue()
            .pop_front()
            .ok_or(ProcessError::Other("no queued status".into()))
    }

    fn kill(&self, _handle: &ProcessHandle, _sig: Signal) -> Result<(), ProcessError> {
        Ok(())
    }

    fn current_process_group(&self) -> ProcessGroupId {
        ProcessGroupId::new(0)
    }

    fn signal_to_number(&self, sig: Signal) -> i32 {
        // Return a stable test value regardless of host platform.
        // Linux numbers (SIGBUS/SIGSTOP differ on macOS/BSD, but the mock
        // only needs self-consistent round-tripping).
        match sig {
            Signal::Hangup => 1,
            Signal::Interrupt => 2,
            Signal::Quit => 3,
            Signal::Illegal => 4,
            Signal::Abort => 6,
            Signal::Bus => 7,
            Signal::FloatingPoint => 8,
            Signal::Kill => 9,
            Signal::Segmentation => 11,
            Signal::Pipe => 13,
            Signal::Alarm => 14,
            Signal::Terminate => 15,
            Signal::Child => 17,
            Signal::Continue => 18,
            Signal::Stop => 19,
            Signal::Tstp => 20,
            Signal::Ttin => 21,
            Signal::Ttou => 22,
            Signal::WindowChange => 28,
            Signal::User1 => 10,
            Signal::User2 => 12,
            Signal::Other(n) => n,
        }
    }

    fn signal_from_number(&self, n: i32) -> Signal {
        match n {
            1 => Signal::Hangup,
            2 => Signal::Interrupt,
            3 => Signal::Quit,
            4 => Signal::Illegal,
            6 => Signal::Abort,
            7 => Signal::Bus,
            8 => Signal::FloatingPoint,
            9 => Signal::Kill,
            10 => Signal::User1,
            11 => Signal::Segmentation,
            12 => Signal::User2,
            13 => Signal::Pipe,
            14 => Signal::Alarm,
            15 => Signal::Terminate,
            17 => Signal::Child,
            18 => Signal::Continue,
            19 => Signal::Stop,
            20 => Signal::Tstp,
            21 => Signal::Ttin,
            22 => Signal::Ttou,
            28 => Signal::WindowChange,
            _ => Signal::Other(n),
        }
    }

    fn signal_name(&self, n: i32) -> &'static str {
        match n {
            1 => "HUP",
            2 => "INT",
            3 => "QUIT",
            4 => "ILL",
            6 => "ABRT",
            7 => "BUS",
            8 => "FPE",
            9 => "KILL",
            10 => "USR1",
            11 => "SEGV",
            12 => "USR2",
            13 => "PIPE",
            14 => "ALRM",
            15 => "TERM",
            17 => "CHLD",
            18 => "CONT",
            19 => "STOP",
            20 => "TSTP",
            21 => "TTIN",
            22 => "TTOU",
            28 => "WINCH",
            _ => "???",
        }
    }

    fn receive_pending_signals(&self) -> Vec<Signal> {
        // Tests can inject signals with `MockPlatform::deliver_signal`.
        let mut pending = self.pending_signals.lock().unwrap();
        core::mem::take(&mut *pending)
    }

    fn parent_pid(&self) -> i32 {
        1
    }

    fn fd_to_path(&self, fd: Fd) -> String {
        format!("/dev/fd/{fd}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_numbers_round_trip() {
        let p = MockPlatform::new();
        // Linux numbers; every symbolic variant maps out and back.
        let cases = [
            (Signal::Hangup, 1, "HUP"),
            (Signal::Interrupt, 2, "INT"),
            (Signal::Quit, 3, "QUIT"),
            (Signal::Illegal, 4, "ILL"),
            (Signal::Abort, 6, "ABRT"),
            (Signal::Bus, 7, "BUS"),
            (Signal::FloatingPoint, 8, "FPE"),
            (Signal::Kill, 9, "KILL"),
            (Signal::User1, 10, "USR1"),
            (Signal::Segmentation, 11, "SEGV"),
            (Signal::User2, 12, "USR2"),
            (Signal::Pipe, 13, "PIPE"),
            (Signal::Alarm, 14, "ALRM"),
            (Signal::Terminate, 15, "TERM"),
            (Signal::Child, 17, "CHLD"),
            (Signal::Continue, 18, "CONT"),
            (Signal::Stop, 19, "STOP"),
            (Signal::Tstp, 20, "TSTP"),
            (Signal::Ttin, 21, "TTIN"),
            (Signal::Ttou, 22, "TTOU"),
            (Signal::WindowChange, 28, "WINCH"),
        ];
        for (sig, num, name) in cases {
            assert_eq!(p.signal_to_number(sig), num);
            assert_eq!(p.signal_from_number(num), sig);
            assert_eq!(p.signal_name(num), name);
        }
        // Unknown numbers survive as Other(n).
        assert_eq!(p.signal_from_number(99), Signal::Other(99));
        assert_eq!(p.signal_name(99), "???");
    }
}
