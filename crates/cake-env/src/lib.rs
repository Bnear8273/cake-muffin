#![no_std]

extern crate alloc;

pub mod env_var;
pub mod stack;

pub use env_var::{EnvVar, EnvVarFlags, PATH_DELIMITER};
pub use stack::{EnvSetError, EnvStack};
