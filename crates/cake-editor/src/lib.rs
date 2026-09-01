//! Pure line-editing state machine (`#![no_std]`). The core builds a buffer
//! from synthetic [`Key`] events and produces a structured [`Render`]; a
//! platform-specific driver feeds decoded keys and dispatches the render.

#![no_std]
extern crate alloc;

pub mod complete;
pub mod editor;
pub mod history;
pub mod keys;
pub mod render;

pub use complete::{CompletionState, longest_common_prefix};
pub use editor::Editor;
pub use history::History;
pub use keys::{CtrlKey, Key, KeyParser};
pub use render::{Render, display_width};

use alloc::string::String;
use alloc::vec::Vec;

/// A completion candidate from the driver.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub display: String,
    pub replacement: String,
}

/// Everything the editor needs from the outside world for one keystroke.
#[allow(clippy::type_complexity)]
pub struct Services<'a> {
    pub prompt: &'a str,
    /// Right-hand prompt content (painted at the end of the first prompt
    /// line in two-line layout); may be empty.
    pub right: &'a str,
    /// Terminal width in columns (0 → 80 fallback inside the renderer).
    pub width: usize,
    /// Syntax-highlight a line (or slice) → ANSI-coloured output.
    pub highlight: &'a dyn Fn(&str) -> String,
    /// Autosuggestion: returns a suffix to show dimly after the cursor when
    /// the cursor is at end-of-line, or `None`.
    pub hint: &'a dyn Fn(&str, &[String]) -> Option<String>,
    /// Tab completion: returns the byte offset of the word being completed
    /// and a list of candidates, or `None` if no completion is applicable.
    pub complete: &'a dyn Fn(&str, usize) -> Option<(usize, Vec<Candidate>)>,
}

/// What one keystroke did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditEvent {
    /// The driver should repaint the editing line.
    Redraw,
    /// The key had no visible effect.
    NoChange,
    /// Enter was pressed; the line is ready for execution.
    Submit(String),
    /// Ctrl-D on an empty line.
    Eof,
    /// Ctrl-C: cancel the current input.
    Interrupted,
}
