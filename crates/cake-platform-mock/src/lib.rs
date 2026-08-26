//! Mock backend for testing. Records spawned commands; returns scripted
//! statuses. No real OS calls.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::vec::Vec;

use cake_platform::{
    Fd, FileInfo, FileOpenMode, Platform, PlatformError, ProcessError, ProcessGroupId,
    ProcessHandle, Signal, SignalMask, SpawnConfig, Termios, TerminalSize, WaitOptions,
    WaitStatus, XdgKind,
};

/// A mock platform that records all spawned commands and returns
/// pre-configured wait statuses.
pub struct MockPlatform {
    spawns: Mutex<Vec<SpawnConfig>>,
    wait_queue: Mutex<VecDeque<WaitStatus>>,
    cwd: Mutex<String>,
    env: Mutex<Vec<(String, String)>>,
    xdg: Mutex<String>,
}

impl MockPlatform {
    pub fn new() -> Self {
        Self {
            spawns: Mutex::new(Vec::new()),
            wait_queue: Mutex::new(VecDeque::new()),
            cwd: Mutex::new("/tmp".into()),
            env: Mutex::new(Vec::new()),
            xdg: Mutex::new("/tmp/.local/share".into()),
        }
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
}

impl Platform for MockPlatform {
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

    fn set_terminal_foreground(&self, _pgid: ProcessGroupId) -> Result<(), PlatformError> {
        Ok(())
    }

    fn current_process_group(&self) -> ProcessGroupId {
        ProcessGroupId::new(0)
    }

    fn install_signal_handler(
        &self,
        _sig: Signal,
        _handler: extern "C" fn(i32),
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    fn block_signals(&self, _sigs: &[Signal]) -> Result<SignalMask, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn unblock_signals(&self, _mask: &SignalMask) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn get_termios(&self, _fd: Fd) -> Result<Termios, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn set_termios(&self, _fd: Fd, _t: &Termios) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn terminal_size(&self, _fd: Fd) -> Result<TerminalSize, PlatformError> {
        Ok(TerminalSize { rows: 24, cols: 80 })
    }

    fn pipe(&self, _cloexec: bool) -> Result<(Fd, Fd), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn open_file(&self, _path: &str, _mode: FileOpenMode) -> Result<Fd, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn dup(&self, _fd: Fd) -> Result<Fd, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn dup2(&self, _oldfd: Fd, _newfd: Fd) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn close(&self, _fd: Fd) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn write(&self, _fd: Fd, _buf: &[u8]) -> Result<usize, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn read(&self, _fd: Fd, _buf: &mut [u8]) -> Result<usize, PlatformError> {
        Err(PlatformError::Unsupported)
    }

    fn run_in_child(&self, _f: &mut dyn FnMut() -> i32) -> Result<ProcessHandle, ProcessError> {
        Err(ProcessError::Other("run_in_child not supported by mock".into()))
    }

    fn is_executable(&self, _path: &str) -> bool {
        true
    }

    fn stat(&self, _path: &str) -> FileInfo {
        FileInfo::default()
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
}