//! Parse a word's raw source text into structured [`WordPart`]s.
//!
//! The lexer has already found the extent of a word and balanced nested
//! `$(...)`, `${...}`, quotes, and backticks. Here we re-scan that text to
//! classify each piece: literals, quotes, expansions, tilde, braces.

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ast::{Parameter, Word, WordPart};
use crate::span::{Offset, Span};

/// Parse `text` (a lexer-produced word) into a [`Word`] with `span`.
///
/// `span` must cover exactly `text`; part spans are derived from it.
pub fn parse_word(text: &str, span: Span) -> Word {
    Word {
        parts: scan(text, span.start, false),
        span,
    }
}

fn scan(text: &str, base: Offset, in_dquotes: bool) -> Vec<WordPart> {
    let mut parts: Vec<WordPart> = Vec::new();
    let mut lit = String::new();
    let mut lit_start: Offset = 0;
    let mut i: usize = 0;
    let n = text.len();

    // Flush accumulated literal text as a `Literal` part ending at `i`.
    macro_rules! flush_lit {
        () => {
            if !lit.is_empty() {
                let start = base + lit_start;
                parts.push(WordPart::Literal(
                    core::mem::take(&mut lit),
                    Span::new(start, base + i as Offset),
                ));
            }
        };
    }

    // Consume one code point at `i`; returns its byte length.
    macro_rules! char_len {
        ($idx:expr) => {{
            let c = text[$idx..].chars().next().unwrap();
            c.len_utf8()
        }};
    }

    while i < n {
        let c = text[i..].chars().next().unwrap();
        match c {
            '\\' => {
                if in_dquotes {
                    // Inside double quotes, backslash only escapes: $ ` " \ newline.
                    // All other characters keep the backslash.
                    let cl = c.len_utf8();
                    if i + cl < n {
                        let next = text[i + cl..].chars().next().unwrap();
                        if matches!(next, '$' | '`' | '"' | '\\') {
                            if lit.is_empty() {
                                lit_start = i as Offset;
                            }
                            lit.push(next);
                            i += cl + next.len_utf8();
                        } else if next == '\n' {
                            // Line continuation: remove both backslash and newline.
                            i += cl + next.len_utf8();
                        } else {
                            // Preserve both backslash and the following char.
                            if lit.is_empty() {
                                lit_start = i as Offset;
                            }
                            lit.push('\\');
                            lit.push(next);
                            i += cl + next.len_utf8();
                        }
                    } else {
                        if lit.is_empty() {
                            lit_start = i as Offset;
                        }
                        lit.push('\\');
                        i += cl;
                    }
                } else {
                    if lit.is_empty() {
                        lit_start = i as Offset;
                    }
                    let cl = c.len_utf8();
                    if i + cl < n {
                        let next = text[i + cl..].chars().next().unwrap();
                        lit.push(next);
                        i += cl + next.len_utf8();
                    } else {
                        lit.push('\\');
                        i += cl;
                    }
                }
            }
            '\'' => {
                flush_lit!();
                match find_simple_quote(text, i + 1, '\'') {
                    Some(close) => {
                        let content = text[i + 1..close].to_owned();
                        parts.push(WordPart::SingleQuoted(
                            content,
                            Span::new(base + i as Offset, base + close as Offset + 1),
                        ));
                        i = close + 1;
                    }
                    None => {
                        // Unterminated (should not reach here from the lexer);
                        // treat as literal.
                        lit.push('\'');
                        lit_start = i as Offset;
                        i += 1;
                    }
                }
            }
            '"' => {
                flush_lit!();
                match find_dquote(text, i + 1) {
                    Some(close) => {
                        let inner = scan(&text[i + 1..close], base + i as Offset + 1, true);
                        parts.push(WordPart::DoubleQuoted(
                            inner,
                            Span::new(base + i as Offset, base + close as Offset + 1),
                        ));
                        i = close + 1;
                    }
                    None => {
                        lit.push('"');
                        lit_start = i as Offset;
                        i += 1;
                    }
                }
            }
            '`' => {
                flush_lit!();
                match find_backtick(text, i + 1) {
                    Some(close) => {
                        let content = text[i + 1..close].to_owned();
                        parts.push(WordPart::CommandSubst(
                            content,
                            Span::new(base + i as Offset, base + close as Offset + 1),
                        ));
                        i = close + 1;
                    }
                    None => {
                        lit.push('`');
                        lit_start = i as Offset;
                        i += 1;
                    }
                }
            }
            '$' => {
                flush_lit!();
                let r = scan_dollar(text, i, base);
                parts.push(r.0);
                i = r.1;
            }
            '~' if i == 0 => {
                flush_lit!();
                // Tilde up to the next `/` or end.
                let mut j = i + 1;
                while j < n {
                    let ch = text[j..].chars().next().unwrap();
                    if ch == '/' {
                        break;
                    }
                    j += ch.len_utf8();
                }
                parts.push(WordPart::Tilde(
                    text[i..j].to_owned(),
                    Span::new(base + i as Offset, base + j as Offset),
                ));
                i = j;
            }
            '{' => {
                // Possible brace expansion; only treated specially when a
                // matching `}` exists (rough check for M1).
                if let Some(close) = find_brace(text, i + 1) {
                    flush_lit!();
                    parts.push(WordPart::Brace(
                        text[i..=close].to_owned(),
                        Span::new(base + i as Offset, base + close as Offset + 1),
                    ));
                    i = close + 1;
                } else {
                    if lit.is_empty() {
                        lit_start = i as Offset;
                    }
                    lit.push(c);
                    i += 1;
                }
            }
            _ => {
                if lit.is_empty() {
                    lit_start = i as Offset;
                }
                lit.push(c);
                i += char_len!(i);
            }
        }
    }
    flush_lit!();
    parts
}

/// Handle a `$` at `i`: returns the part and the new byte offset.
fn scan_dollar(text: &str, i: usize, base: Offset) -> (WordPart, usize) {
    let cl = 1; // '$'
    let next_i = i + cl;
    if next_i >= text.len() {
        return (WordPart::Literal("$".into(), Span::new(base + i as Offset, base + next_i as Offset)), next_i);
    }
    let c = text[next_i..].chars().next().unwrap();
    match c {
        '(' => {
            if text[next_i + 1..].starts_with('(') {
                // $(( ... ))
                match find_arith(text, next_i + 2) {
                    Some(close) => {
                        let content = text[next_i + 2..close - 1].to_owned();
                        (
                            WordPart::ArithExpansion(
                                content,
                                Span::new(base + i as Offset, base + close as Offset + 1),
                            ),
                            close + 1,
                        )
                    }
                    None => (
                        WordPart::Literal("$".into(), Span::new(base + i as Offset, base + next_i as Offset)),
                        next_i,
                    ),
                }
            } else {
                // $( ... )
                match find_command_subst(text, next_i + 1) {
                    Some(close) => {
                        let content = text[next_i + 1..close].to_owned();
                        (
                            WordPart::CommandSubst(
                                content,
                                Span::new(base + i as Offset, base + close as Offset + 1),
                            ),
                            close + 1,
                        )
                    }
                    None => (
                        WordPart::Literal("$".into(), Span::new(base + i as Offset, base + next_i as Offset)),
                        next_i,
                    ),
                }
            }
        }
        '{' => {
            match find_braced(text, next_i + 1) {
                Some(close) => {
                    let inner = &text[next_i + 1..close];
                    let name = braced_name(inner).to_owned();
                    (
                        WordPart::Parameter(
                            Parameter {
                                name,
                                braced: true,
                                text: text[i..=close].to_owned(),
                            },
                            Span::new(base + i as Offset, base + close as Offset + 1),
                        ),
                        close + 1,
                    )
                }
                None => (
                    WordPart::Literal("$".into(), Span::new(base + i as Offset, base + next_i as Offset)),
                    next_i,
                ),
            }
        }
        '\'' => {
match find_simple_quote(text, next_i + 1, '\'') {
                    Some(close) => {
                        let content = text[next_i + 1..close].to_owned();
                    (
                        WordPart::AnsiCQuoted(
                            content,
                            Span::new(base + i as Offset, base + close as Offset + 1),
                        ),
                        close + 1,
                    )
                }
                None => (
                    WordPart::Literal("$".into(), Span::new(base + i as Offset, base + next_i as Offset)),
                    next_i,
                ),
            }
        }
        '"' => {
            // $"..." — locale quoting, treat as a double-quoted string.
            match find_dquote(text, next_i + 1) {
                Some(close) => {
                    let inner = scan(&text[next_i + 1..close], base + next_i as Offset + 1, true);
                    (
                        WordPart::DoubleQuoted(
                            inner,
                            Span::new(base + i as Offset, base + close as Offset + 1),
                        ),
                        close + 1,
                    )
                }
                None => (
                    WordPart::Literal("$".into(), Span::new(base + i as Offset, base + next_i as Offset)),
                    next_i,
                ),
            }
        }
        _ => {
            // $name, $1, $?, ...
            let mut j = next_i;
            let mut name = String::new();
            let ch = text[j..].chars().next().unwrap();
            if ch == '_' || ch.is_ascii_alphabetic() {
                while j < text.len() {
                    let cc = text[j..].chars().next().unwrap();
                    if cc == '_' || cc.is_ascii_alphanumeric() {
                        name.push(cc);
                        j += cc.len_utf8();
                    } else {
                        break;
                    }
                }
            } else if ch.is_ascii_digit() {
                name.push(ch);
                j += ch.len_utf8();
            } else {
                // Special parameter: ? $ ! # @ * 0 - etc.
                name.push(ch);
                j += ch.len_utf8();
            }
            let span = Span::new(base + i as Offset, base + j as Offset);
            (
                WordPart::Parameter(
                    Parameter {
                        name,
                        braced: false,
                        text: text[i..j].to_owned(),
                    },
                    span,
                ),
                j,
            )
        }
    }
}

/// Extract the base variable name from `${...}` content (before any `:`/`#`/
/// `/`/`[` operator). Returns `""` for `${...}` special forms.
fn braced_name(inner: &str) -> &str {
    for (idx, ch) in inner.char_indices() {
        if matches!(ch, ':' | '#' | '%' | '/' | '[' | '}' | '=' | '+' | '?' | '-' | '^' | ',' | '!' | '@') {
            return &inner[..idx];
        }
    }
    inner
}

// --- nested-construct scanners ---
//
// Each returns the byte offset of the closing delimiter, or None.
// The `start` argument points at the first character AFTER the opener.

/// Find the closing `quote` (no escapes inside single quotes).
fn find_simple_quote(text: &str, start: usize, quote: char) -> Option<usize> {
    for (idx, ch) in text[start..].char_indices() {
        if ch == quote {
            return Some(start + idx);
        }
    }
    None
}

/// Find the closing `"` starting just after an opening `"`.
fn find_dquote(text: &str, start: usize) -> Option<usize> {
    let mut i = start;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        match c {
            '\\' => {
                i += c.len_utf8();
                if i < text.len() {
                    i += text[i..].chars().next().unwrap().len_utf8();
                }
            }
            '$' => {
                let cl = c.len_utf8();
                if i + cl < text.len() {
                    let nc = text[i + cl..].chars().next().unwrap();
                    if nc == '(' {
                        if text[i + cl + 1..].starts_with('(') {
                            if let Some(e) = find_arith(text, i + cl + 2) {
                                i = e + 1;
                                continue;
                            }
                        } else if let Some(e) = find_command_subst(text, i + cl + 1) {
                            i = e + 1;
                            continue;
                        }
                    } else if nc == '{' {
                        if let Some(e) = find_braced(text, i + cl + 1) {
                            i = e + 1;
                            continue;
                        }
                    }
                }
                i += cl;
            }
            '`' => {
                if let Some(e) = find_backtick(text, i + 1) {
                    i = e + 1;
                } else {
                    i += 1;
                }
            }
            '"' => return Some(i),
            _ => i += c.len_utf8(),
        }
    }
    None
}

/// Find the closing backtick starting just after an opening backtick.
fn find_backtick(text: &str, start: usize) -> Option<usize> {
    let mut i = start;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        if c == '\\' {
            i += c.len_utf8();
            if i < text.len() {
                i += text[i..].chars().next().unwrap().len_utf8();
            }
        } else if c == '`' {
            return Some(i);
        } else {
            i += c.len_utf8();
        }
    }
    None
}

/// Find the matching `)` for a `$( ... )` substitution, starting just after
/// the opening `(`.
fn find_command_subst(text: &str, start: usize) -> Option<usize> {
    let mut depth: i32 = 1;
    let mut i = start;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            '\\' => {
                i += c.len_utf8();
                if i < text.len() {
                    i += text[i..].chars().next().unwrap().len_utf8();
                }
                continue;
            }
            '\'' => {
                if let Some(e) = find_simple_quote(text, i + 1, '\'') {
                    i = e + 1;
                    continue;
                }
            }
            '"' => {
                if let Some(e) = find_dquote(text, i + 1) {
                    i = e + 1;
                    continue;
                }
            }
            '`' => {
                if let Some(e) = find_backtick(text, i + 1) {
                    i = e + 1;
                    continue;
                }
            }
            '$' => {
                let cl = c.len_utf8();
                if i + cl < text.len() {
                    let nc = text[i + cl..].chars().next().unwrap();
                    if nc == '(' {
                        if text[i + cl + 1..].starts_with('(') {
                            if let Some(e) = find_arith(text, i + cl + 2) {
                                i = e + 1;
                                continue;
                            }
                        } else if let Some(e) = find_command_subst(text, i + cl + 1) {
                            i = e + 1;
                            continue;
                        }
                    } else if nc == '{' {
                        if let Some(e) = find_braced(text, i + cl + 1) {
                            i = e + 1;
                            continue;
                        }
                    }
                }
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

/// Find the `))` closing a `$(( ... ))`, starting just after the opening `((`.
fn find_arith(text: &str, start: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut i = start;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        match c {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    if text[i + 1..].starts_with(')') {
                        return Some(i + 1);
                    }
                    return None;
                }
                depth -= 1;
            }
            '\'' => {
                if let Some(e) = find_simple_quote(text, i + 1, '\'') {
                    i = e + 1;
                    continue;
                }
            }
            '"' => {
                if let Some(e) = find_dquote(text, i + 1) {
                    i = e + 1;
                    continue;
                }
            }
            '`' => {
                if let Some(e) = find_backtick(text, i + 1) {
                    i = e + 1;
                    continue;
                }
            }
            '$' => {
                let cl = c.len_utf8();
                if i + cl < text.len() && text[i + cl..].starts_with('(')
                    && text[i + cl + 1..].starts_with('(')
                {
                    if let Some(e) = find_arith(text, i + cl + 2) {
                        i = e + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

/// Find the matching `}` for a `${ ... }`, starting just after the `{`.
fn find_braced(text: &str, start: usize) -> Option<usize> {
    let mut depth: i32 = 1;
    let mut i = start;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        match c {
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            '$' => {
                let cl = c.len_utf8();
                if i + cl < text.len() && text[i + cl..].starts_with('{') {
                    if let Some(e) = find_braced(text, i + cl + 1) {
                        i = e + 1;
                        continue;
                    }
                }
            }
            '\\' => {
                i += c.len_utf8();
                if i < text.len() {
                    i += text[i..].chars().next().unwrap().len_utf8();
                }
                continue;
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

/// Roughly find the matching `}` for brace expansion `{a,b}`. Stops at a
/// comma-only heuristic; M2 refines expansion semantics.
fn find_brace(text: &str, start: usize) -> Option<usize> {
    let mut depth: i32 = 1;
    let mut i = start;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            '\\' => {
                i += c.len_utf8();
                if i < text.len() {
                    i += text[i..].chars().next().unwrap().len_utf8();
                }
                continue;
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}
