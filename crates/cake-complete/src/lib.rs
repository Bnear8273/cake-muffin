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

/// Find the `$` that starts the variable name being typed in `word`, if any.
///
/// The tail after the `$` must be a plausible variable-name prefix: empty
/// (bare `$`), or all alphanumerics/underscores, possibly closed by a
/// trailing quote. Returns the byte offset of the `$` within `word`, taking
/// the `$` closest to the cursor (last in the word).
pub fn dollar_in_word(word: &str) -> Option<usize> {
    word.char_indices()
        .rev()
        .find(|(i, c)| {
            if *c != '$' {
                return false;
            }
            let tail = word[*i + c.len_utf8()..].trim_end_matches(['"', '\'']);
            tail.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
        .map(|(i, _)| i)
}

/// Classify what the word under the cursor is completing.
pub fn classify(line: &str, pos: usize) -> CompleteKind {
    let (_, word) = current_word(line, pos);
    // `$name` (or a bare `$`) → variable completion.
    if dollar_in_word(word).is_some() {
        return CompleteKind::Variable;
    }

    // Drive the lexer over the text before the cursor to learn the context
    // at the cursor position.
    let before = &line[..pos.min(line.len())];
    let mut lexer = Lexer::new(before);
    lexer.ctx = LexContext {
        cmd_pos: true,
        ..Default::default()
    };
    // The context just before the last real token, and whether that token is
    // a (possibly partial) word ending exactly at the cursor.
    let mut ctx_before_last: Option<LexContext> = None;
    let mut last_is_word_like = false;
    let mut last_end = 0usize;
    let mut saw_token = false;
    while let Ok(tok) = lexer.next_token() {
        if tok.kind == Eof {
            break;
        }
        saw_token = true;
        ctx_before_last = Some(lexer.ctx);
        last_end = tok.span.end as usize;
        last_is_word_like = matches!(tok.kind, Word | Assignment);
        update_ctx(&mut lexer, &tok);
    }

    // If the cursor sits at the end of a word being typed, the relevant
    // context is the one *before* that word; otherwise (trailing operator or
    // whitespace) it is the context *after* the last token.
    let ctx_at_cursor = if saw_token && last_is_word_like && last_end == before.len() {
        ctx_before_last.unwrap_or(lexer.ctx)
    } else {
        lexer.ctx
    };

    if ctx_at_cursor.in_redir {
        CompleteKind::File
    } else if ctx_at_cursor.cmd_pos {
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

fn update_ctx(lexer: &mut Lexer, tok: &cake_syntax::Token) {
    match tok.kind {
        Word => {
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
        Assignment => {
            lexer.ctx = LexContext {
                cmd_pos: true,
                ..Default::default()
            };
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
        assert_eq!(classify("$", 1), CompleteKind::Variable);
        assert_eq!(classify("ls | ", 5), CompleteKind::Command);
        assert_eq!(classify("cat < ", 6), CompleteKind::File);
        assert_eq!(classify("git status && ", 14), CompleteKind::Command);
        assert_eq!(classify("echo \"$HO", 9), CompleteKind::Variable);
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
}
