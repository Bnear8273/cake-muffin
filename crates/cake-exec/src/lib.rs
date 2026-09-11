//! Cake-shell evaluator.
//!
//! M2: the full bash-semantics executor — control flow, expansions,
//! redirections, pipelines, builtins and functions.

#![no_std]

extern crate alloc;

pub mod arith;
pub mod builtins;
pub mod cond;
pub mod executor;
pub mod expand;
pub mod glob;
pub mod path;
pub mod proc_status;
pub mod redirect;
pub mod resolve;
mod test_ops;

pub use executor::{EvalOutcome, Executor};
pub use path::find_in_path;
pub use proc_status::ProcStatus;
