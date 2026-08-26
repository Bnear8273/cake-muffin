use alloc::string::String;

use crate::span::{Offset, Span};

/// A syntax error with source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
    /// Set when the input ended in the middle of an unfinished construct
    /// (unclosed `if`, `(`, quote, heredoc, ...). Interactive readers use
    /// this to show a continuation prompt instead of an error.
    pub is_incomplete: bool,
}

impl ParseError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
            is_incomplete: false,
        }
    }

    /// An "unexpected end of input" style error, which interactive shells
    /// treat as "keep reading".
    pub fn incomplete(message: impl Into<String>, pos: Offset) -> Self {
        Self {
            message: message.into(),
            span: Span::at(pos),
            is_incomplete: true,
        }
    }
}
