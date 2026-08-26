use alloc::vec::Vec;

use crate::proc::{ProcStatus, Process};

/// Flags describing job-level properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JobFlags(u8);

impl JobFlags {
    pub const NONE: JobFlags = JobFlags(0);
    /// The job is negated (`! cmd`).
    pub const NEGATE: JobFlags = JobFlags(1 << 0);
    /// The job is a pipeline root (owns a process group).
    pub const IS_GROUP_ROOT: JobFlags = JobFlags(1 << 1);
}

/// A job: one or more processes joined by pipes.
#[derive(Debug, Clone)]
pub struct Job {
    pub processes: Vec<Process>,
    /// Monotonically increasing id for `jobs` output.
    pub job_id: usize,
    pub is_background: bool,
    pub flags: JobFlags,
}

impl Job {
    pub fn new(job_id: usize) -> Self {
        Self {
            processes: Vec::new(),
            job_id,
            is_background: false,
            flags: JobFlags::NONE,
        }
    }

    /// True when every process has produced a final status.
    pub fn is_completed(&self) -> bool {
        self.processes.iter().all(|p| p.status.is_done())
    }

    /// The status of the job: that of its last process (bash `$?`).
    pub fn status(&self) -> ProcStatus {
        self.processes
            .iter()
            .rev()
            .find(|p| p.status.is_done())
            .map(|p| p.status)
            .unwrap_or(ProcStatus::NotStarted)
    }

    pub fn push_process(&mut self, process: Process) {
        self.processes.push(process);
    }
}
