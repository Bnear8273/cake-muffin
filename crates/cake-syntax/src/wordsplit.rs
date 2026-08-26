//! A minimal, temporary quote-aware word splitter.
//!
//! Handles whitespace separation, single quotes, double quotes (with `$`,
//! backtick and backslash escapes recognized but not expanded), and backslash
//! escapes. No operators (`;`, `|`, `&&`), no expansion, no redirections —
//! all of that is the job of the full parser in M1.

use alloc::string::String;
use alloc::vec::Vec;

/// Split `src` into argv-like words.
///
/// Returns `Err` on an unterminated quote so callers can show a friendly
/// error instead of silently dropping the rest of the line.
pub fn split_command_line(src: &str) -> Result<Vec<String>, ()> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut have_token = false;
    let mut chars = src.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                have_token = true;
                let mut closed = false;
                while let Some(&c2) = chars.peek() {
                    chars.next();
                    if c2 == '\'' {
                        closed = true;
                        break;
                    }
                    cur.push(c2);
                }
                if !closed {
                    return Err(());
                }
            }
            '"' => {
                have_token = true;
                let mut closed = false;
                while let Some(&c2) = chars.peek() {
                    chars.next();
                    match c2 {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => {
                            if let Some(&c3) = chars.peek() {
                                if matches!(c3, '$' | '`' | '"' | '\\') {
                                    chars.next();
                                    cur.push(c3);
                                } else {
                                    cur.push('\\');
                                }
                            }
                        }
                        _ => cur.push(c2),
                    }
                }
                if !closed {
                    return Err(());
                }
            }
            '\\' => {
                have_token = true;
                if let Some(&c2) = chars.peek() {
                    chars.next();
                    cur.push(c2);
                } else {
                    cur.push('\\');
                }
            }
            c if c.is_whitespace() => {
                if have_token {
                    args.push(core::mem::take(&mut cur));
                    have_token = false;
                }
            }
            c => {
                have_token = true;
                cur.push(c);
            }
        }
    }

    if have_token {
        args.push(cur);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_simple() {
        assert_eq!(split_command_line("echo hello world").unwrap(), ["echo", "hello", "world"]);
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(split_command_line("  echo   hi  ").unwrap(), ["echo", "hi"]);
    }

    #[test]
    fn single_quotes() {
        assert_eq!(split_command_line("echo 'hello world'").unwrap(), ["echo", "hello world"]);
        assert_eq!(split_command_line("echo 'it''s'").unwrap(), ["echo", "its"]);
    }

    #[test]
    fn double_quotes() {
        assert_eq!(split_command_line("echo \"hello world\"").unwrap(), ["echo", "hello world"]);
        assert_eq!(split_command_line("echo \"a\\\"b\"").unwrap(), ["echo", "a\"b"]);
    }

    #[test]
    fn backslash_escape() {
        assert_eq!(split_command_line("echo a\\ b").unwrap(), ["echo", "a b"]);
    }

    #[test]
    fn empty_input() {
        assert_eq!(split_command_line("").unwrap(), Vec::<String>::new());
        assert_eq!(split_command_line("   ").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn unterminated_quote_errors() {
        assert!(split_command_line("echo 'oops").is_err());
        assert!(split_command_line("echo \"oops").is_err());
    }
}
