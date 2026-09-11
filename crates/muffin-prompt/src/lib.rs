//! p10k-style segmented prompt (`#![no_std]`).
//!
//! Pure logic only: parses the `MUFFIN_PROMPT_*` configuration from
//! environment-variable strings, parses `git status --porcelain=v2 -b`
//! output, and renders a colour-segmented ANSI prompt from injected facts.
//! The driver (`muffin/src/prompt.rs`) gathers the facts and spawns `git`.

#![no_std]

extern crate alloc;

pub mod config;
pub mod git;
pub mod render;

pub use config::{Color, Config, Segment, TIME_DEFAULT_MS};
pub use git::{GitStatus, parse_status};
pub use render::{Facts, Rendered, render};
