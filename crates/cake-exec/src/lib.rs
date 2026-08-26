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
pub mod redirect;
pub mod resolve;

pub use executor::{EvalOutcome, Executor};
pub use path::find_in_path;
