//! Tab-completion logic (pure, `#![no_std]`).
//!
//! The shell driver gathers candidate sources (PATH executables, directory
//! listings, variable names) and asks this crate which candidates match the
//! word under the cursor and what kind of completion is expected.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use cake_syntax::lexer::{LexContext, Lexer};
use cake_syntax::token::TokenKind::*;

/// What is being completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteKind {
    /// A command name (first word of a command).
    Command,
    /// A file/path argument.
    File,
    /// A variable name after `$`.
    Variable,
}

/// The word currently under the cursor and where it starts.
///
/// `pos` is a byte offset into `line`.
pub fn current_word(line: &str, pos: usize) -> (usize, &str) {
    let before = &line[..pos.min(line.len())];
    let start = before
        .rfind(|c: char| c.is_whitespace() || ";&|(){}<>".contains(c))
        .map(|i| i + 1)
        .unwrap_or(0);
    (start, &before[start..])
}

/// Classify what the word under the cursor is completing.
pub fn classify(line: &str, pos: usize) -> CompleteKind {
    let (_, word) = current_word(line, pos);
    // `$name` → variable completion.
    let trimmed = word.trim_start_matches('"').trim_start_matches('\'');
    if trimmed.starts_with('$') && trimmed.len() > 1 {
        return CompleteKind::Variable;
    }

    // Drive the lexer over the text before the cursor to learn whether the
    // current position is in command position.
    let before = &line[..pos.min(line.len())];
    let mut lexer = Lexer::new(before);
    lexer.ctx = LexContext {
        cmd_pos: true,
        ..Default::default()
    };
    let mut was_cmd = true;
    loop {
        let cmd_pos_before = lexer.ctx.cmd_pos;
        match lexer.next_token() {
            Ok(tok) => {
                if tok.kind == Eof {
                    break;
                }
                was_cmd = cmd_pos_before;
                update_ctx(&mut lexer, &tok);
            }
            Err(_) => break,
        }
    }

    if was_cmd {
        CompleteKind::Command
    } else {
        CompleteKind::File
    }
}

/// Filter candidate strings to those starting with `prefix`.
pub fn filter_candidates(prefix: &str, candidates: &[String]) -> Vec<String> {
    candidates
        .iter()
        .filter(|c| c.starts_with(prefix))
        .cloned()
        .collect()
}

/// The longest common prefix of a set of candidates.
pub fn common_prefix(candidates: &[String]) -> String {
    let Some(first) = candidates.first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for c in &candidates[1..] {
        while !c.starts_with(&prefix) {
            prefix.pop();
        }
    }
    prefix
}

fn update_ctx(lexer: &mut Lexer, tok: &cake_syntax::Token) {
    match tok.kind {
        Word | Assignment => {
            let kw = tok.text.as_str();
            if matches!(
                kw,
                "if" | "then" | "else" | "elif" | "for" | "do" | "while" | "until" | "case"
                    | "function" | "select" | "time" | "in" | "!"
            ) {
                lexer.ctx = LexContext {
                    cmd_pos: true,
                    ..Default::default()
                };
            } else {
                lexer.ctx = LexContext {
                    cmd_pos: false,
                    ..Default::default()
                };
            }
        }
        Lbrace | Lparen | ArithOpen | DoubleBracketOpen | Semi | SemiSemi | Amp | Newline
        | Pipe | PipeAmp | AndAnd | OrOr | Bang => {
            lexer.ctx = LexContext {
                cmd_pos: true,
                ..Default::default()
            };
        }
        Greater | GreatGreat | Less | LessLess | LessLessLess | LessLessDash | GreaterAnd
        | LessAnd | AmpGreat | AmpGreatGreat | GreatBar | LessGreat | LessGreatGreat => {
            lexer.ctx = LexContext {
                cmd_pos: true,
                in_redir: true,
                ..Default::default()
            };
        }
        _ => {
            lexer.ctx = LexContext {
                cmd_pos: false,
                ..Default::default()
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn classifies_command_position() {
        assert_eq!(classify("", 0), CompleteKind::Command);
        assert_eq!(classify("ec", 2), CompleteKind::Command);
        assert_eq!(classify("ls -l /tm", 10), CompleteKind::File);
        assert_eq!(classify("if tru", 6), CompleteKind::Command);
        assert_eq!(classify("echo $HO", 8), CompleteKind::Variable);
    }

    #[test]
    fn current_word_extraction() {
        assert_eq!(current_word("echo hel", 8), (5, "hel"));
        assert_eq!(current_word("ls", 2), (0, "ls"));
        assert_eq!(current_word("a b c", 5), (4, "c"));
    }

    #[test]
    fn filters_candidates() {
        let all = s(&["apple", "apricot", "banana", "cherry"]);
        let got = filter_candidates("ap", &all);
        assert_eq!(got, s(&["apple", "apricot"]));
    }

    #[test]
    fn computes_common_prefix() {
        let cands = s(&["apple", "apricot"]);
        assert_eq!(common_prefix(&cands), "ap");
        assert_eq!(common_prefix(&[]), "");
    }
}
