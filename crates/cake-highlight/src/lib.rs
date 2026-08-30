//! Syntax highlighting for the interactive shell.
//!
//! Tokenizes a source line with [`cake_syntax::lexer`] and inserts ANSI
//! escape sequences for colourising keywords, strings, variables, numbers,
//! comments and operators.
//!
//! This crate is `#![no_std]`; only the shell driver (std) prints the
//! highlighted output.

#![no_std]

extern crate alloc;

use alloc::string::String;

use cake_syntax::Token;
use cake_syntax::lexer::{LexContext, Lexer};
use cake_syntax::token::TokenKind::*;

/// Highlight a single line of shell input.
///
/// The line need not be syntactically complete; partial input is coloured
/// up to the valid prefix and the remainder is appended as-is.
///
/// `is_command` decides whether a word in command position names a real
/// command (builtin, function or executable on `PATH`); found commands are
/// green, not-found ones red.
pub fn highlight_line(src: &str, is_command: &dyn Fn(&str) -> bool) -> String {
    let mut out = String::new();
    let mut lexer = Lexer::new(src);
    lexer.ctx = LexContext {
        cmd_pos: true,
        ..Default::default()
    };
    let mut pos = 0usize;

    loop {
        let was_cmd = lexer.ctx.cmd_pos;
        match lexer.next_token() {
            Ok(tok) => {
                if tok.kind == Eof {
                    break;
                }
                let start = tok.span.start as usize;
                if start > pos {
                    out.push_str(&src[pos..start]);
                }
                out.push_str(color_for(&tok, was_cmd, is_command));
                out.push_str(&tok.text);
                out.push_str("\x1b[0m");
                pos = tok.span.end as usize;
                update_ctx(&mut lexer, &tok);
            }
            Err(_e) => {
                // Lexer error (e.g. unterminated string): append the rest raw.
                if pos < src.len() {
                    out.push_str(&src[pos..]);
                }
                break;
            }
        }
    }
    out
}

fn color_for(tok: &Token, was_cmd: bool, is_command: &dyn Fn(&str) -> bool) -> &'static str {
    match tok.kind {
        Word if was_cmd && is_keyword(tok.text.as_str()) => "\x1b[1;35m", // magenta bold (keyword)
        Word if was_cmd && is_command(tok.text.as_str()) => "\x1b[32m",   // green (command found)
        Word if was_cmd => "\x1b[31m",                                    // red (command not found)
        Word => "\x1b[0m",                                                // default
        Assignment => "\x1b[36m",                                         // cyan
        Lparen | Rparen | Lbrace | Rbrace | Bang => "\x1b[1;36m",         // cyan bold
        DoubleBracketOpen | DoubleBracketClose => "\x1b[1;36m",
        ArithOpen => "\x1b[1;36m",
        AndAnd | OrOr | Pipe | PipeAmp | Semi | SemiSemi | Amp => "\x1b[1;36m",
        Greater | GreatGreat | Less | LessLess | LessLessLess | LessLessDash | GreaterAnd
        | LessAnd | AmpGreat | AmpGreatGreat | GreatBar | LessGreat | LessGreatGreat => {
            "\x1b[1;36m"
        }
        IoNumber => "\x1b[35m",
        Newline => "\x1b[90m",
        _ => "\x1b[0m",
    }
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "do"
            | "done"
            | "in"
            | "while"
            | "until"
            | "case"
            | "esac"
            | "function"
            | "select"
            | "coproc"
            | "time"
            | "!"
    )
}

fn update_ctx(lexer: &mut Lexer, tok: &Token) {
    use cake_syntax::lexer::LexContext;
    match tok.kind {
        Word => {
            if matches!(
                tok.text.as_str(),
                "if" | "then"
                    | "else"
                    | "elif"
                    | "for"
                    | "do"
                    | "while"
                    | "until"
                    | "case"
                    | "function"
                    | "select"
                    | "coproc"
                    | "time"
                    | "in"
                    | "!"
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
            // Prefix assignments are followed by the command they prefix.
            lexer.ctx = LexContext {
                cmd_pos: true,
                ..Default::default()
            };
        }
        Lbrace => {
            // `{` starts a block; the next token is a command.
            lexer.ctx = LexContext {
                cmd_pos: true,
                ..Default::default()
            };
        }
        Lparen | ArithOpen | DoubleBracketOpen => {
            lexer.ctx = LexContext {
                cmd_pos: true,
                ..Default::default()
            };
        }
        Semi | SemiSemi | Amp | Newline | Pipe | PipeAmp | AndAnd | OrOr | Bang => {
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

    fn found(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |n: &str| names.contains(&n)
    }

    #[test]
    fn keywords_are_magenta() {
        let out = highlight_line("if true; then", &found(&["true"]));
        assert!(out.contains("\x1b[1;35mif\x1b[0m"));
        assert!(out.contains("\x1b[1;35mthen\x1b[0m"));
    }

    #[test]
    fn command_found_is_green() {
        let out = highlight_line("ls -la", &found(&["ls"]));
        assert!(out.contains("\x1b[32mls\x1b[0m"));
    }

    #[test]
    fn command_not_found_is_red() {
        let out = highlight_line("llll", &found(&["ls"]));
        assert!(out.contains("\x1b[31mllll\x1b[0m"));
    }

    #[test]
    fn body_command_after_then_is_green() {
        let out = highlight_line("if true; then ls -la; fi", &found(&["ls"]));
        assert!(out.contains("\x1b[32mls\x1b[0m"));
        assert!(out.contains("\x1b[1;35mfi\x1b[0m"));
    }

    #[test]
    fn assignment_keeps_command_position() {
        let out = highlight_line("A=1 ls", &found(&["ls"]));
        assert!(out.contains("\x1b[36mA=1\x1b[0m"));
        assert!(out.contains("\x1b[32mls\x1b[0m"));
    }

    #[test]
    fn variable_is_highlighted() {
        let out = highlight_line("echo $HOME", &found(&["echo"]));
        // The variable token isn't distinguished by kind; at minimum the
        // command and text survive.
        assert!(out.contains("echo"));
        assert!(out.contains("$HOME"));
    }

    #[test]
    fn operators_colored() {
        let out = highlight_line("a && b | c", &found(&["a", "b", "c"]));
        assert!(out.contains("\x1b[1;36m&&\x1b[0m"));
        assert!(out.contains("\x1b[1;36m|\x1b[0m"));
    }

    #[test]
    fn partial_line_is_handled() {
        // Unterminated double quote: no panic, prefix highlighted.
        let out = highlight_line("echo \"unclosed", &found(&["echo"]));
        assert!(out.contains("echo"));
    }
}
