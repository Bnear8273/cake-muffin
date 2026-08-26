#![no_std]

extern crate alloc;

pub mod job;
pub mod proc;

pub use job::{Job, JobFlags};
pub use proc::{ProcStatus, Process, ProcessType};
