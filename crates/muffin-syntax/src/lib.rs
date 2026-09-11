//! Cake-shell syntax: lexer + parser + AST.
//!
//! Pure logic, no OS dependency: this crate is `#![no_std]` and only uses
//! `alloc`. M1 will replace this module with a full recursive-descent bash
//! grammar parser. For now it provides a small quote-aware word splitter used
//! by the M0 tracer bullet so that `muffin -c "echo hello"` works end-to-end.

#![no_std]

extern crate alloc;

pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;
pub mod word;
pub mod wordsplit;

pub use ast::*;
pub use error::ParseError;
pub use parser::parse;
pub use span::{Offset, Span};
pub use token::{Token, TokenKind};
pub use wordsplit::split_command_line;
