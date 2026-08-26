//! Word expansion.
//!
//! M2a implements: tilde expansion, parameter expansion (`$var`, `${var}`,
//! `$?`, `$#`, `$@`, `$*`, `$$`), quote removal, and field splitting on IFS.
//! Command substitution, arithmetic expansion, brace expansion and globbing
//! arrive in M2c.

use alloc::borrow::ToOwned;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use cake_env::EnvStack;
use cake_proc::ProcStatus;
use cake_syntax::{Parameter, Word, WordPart};

/// Context for expanding a word.
pub struct ExpandCtx<'a> {
    pub env: &'a EnvStack,
    pub last_status: ProcStatus,
    /// Positional parameters (`$1`, `$@`, ...).
    pub positional: &'a [String],
    /// The shell's own pid for `$$`.
    pub shell_pid: i32,
}

/// The characters that make up IFS by default when IFS is unset.
const DEFAULT_IFS: &str = " \t\n";

fn ifs_chars(env: &EnvStack) -> Vec<char> {
    match env.get("IFS") {
        Some(var) => var.value().chars().collect(),
        None => DEFAULT_IFS.chars().collect(),
    }
}

/// Expand a single word into zero or more fields.
///
/// Empty results are dropped, so `""` / `$unset` in a word can yield zero
/// fields. Each returned string is one argv element.
pub fn expand_word(ctx: &ExpandCtx, word: &Word) -> Result<Vec<String>, String> {
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let ifs = ifs_chars(ctx.env);

    for part in &word.parts {
        match expand_part(ctx, part, false, &ifs)? {
            PartOut::Append(s) => {
                cur.push_str(&s);
            }
            PartOut::Split(s) => {
                split_fields(&mut fields, &mut cur, &s, &ifs);
            }
            PartOut::Fields(v) => {
                if !cur.is_empty() || !v.is_empty() {
                    if !cur.is_empty() {
                        fields.push(core::mem::take(&mut cur));
                    }
                    for f in v {
                        fields.push(f);
                    }
                }
            }
            PartOut::Nothing => {}
        }
    }

    if !cur.is_empty() {
        fields.push(cur);
    }
    Ok(fields)
}

/// Expand a word inside double quotes: the result is always a single field
/// (no splitting), except for `$@` which expands to one field per positional
/// parameter.
pub fn expand_word_quoted(ctx: &ExpandCtx, word: &Word) -> Result<String, String> {
    let mut out = String::new();
    for part in &word.parts {
        match expand_part(ctx, part, true, &ifs_chars(ctx.env))? {
            PartOut::Append(s) | PartOut::Split(s) => out.push_str(&s),
            // `"$@"` inside a quoted string: join params with spaces
            // (approximates bash; exact behaviour only matters with 0 params).
            PartOut::Fields(v) => {
                for (i, f) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    out.push_str(f);
                }
            }
            PartOut::Nothing => {}
        }
    }
    Ok(out)
}

enum PartOut {
    /// Text that is appended to the current field verbatim.
    Append(String),
    /// A value subject to IFS field splitting (unquoted expansion).
    Split(String),
    /// One or more ready-made fields (quoted `$@`).
    Fields(Vec<String>),
    /// Produces nothing (unset variable in unquoted position).
    Nothing,
}

fn expand_part(ctx: &ExpandCtx, part: &WordPart, in_dquotes: bool, ifs: &[char]) -> Result<PartOut, String> {
    match part {
        WordPart::Literal(s, _) => Ok(PartOut::Append(s.clone())),
        WordPart::SingleQuoted(s, _) => Ok(PartOut::Append(s.clone())),
        WordPart::AnsiCQuoted(s, _) => Ok(PartOut::Append(ansi_c_decode(s)?)),
        WordPart::DoubleQuoted(parts, _) => {
            let mut out = String::new();
            for p in parts {
                match expand_part(ctx, p, true, ifs)? {
                    PartOut::Append(s) | PartOut::Split(s) => out.push_str(&s),
                    PartOut::Fields(v) => {
                        for (i, f) in v.iter().enumerate() {
                            if i > 0 {
                                out.push(' ');
                            }
                            out.push_str(f);
                        }
                    }
                    PartOut::Nothing => {}
                }
            }
            Ok(PartOut::Append(out))
        }
        WordPart::Parameter(p, _) => expand_parameter(ctx, p, in_dquotes),
        WordPart::Tilde(text, _) => Ok(PartOut::Append(expand_tilde(ctx.env, text))),
        // Command substitution, arithmetic expansion, brace expansion,
        // process substitution: full support in M2c. For now expand the
        // inside text literally so `$((1+2))` still yields something useful.
        WordPart::CommandSubst(s, _) => Ok(PartOut::Append(s.clone())),
        WordPart::ArithExpansion(s, _) => {
            // bash: parameter expansion happens on the arithmetic text first
            // (`$(( $1 + $2 ))`), then the result is evaluated.
            let expanded = expand_arith_text(ctx, s)?;
            let v = crate::arith::eval_arith_value(ctx.env, &expanded)?;
            Ok(if in_dquotes {
                PartOut::Append(v)
            } else {
                PartOut::Split(v)
            })
        }
        WordPart::Brace(s, _) => Ok(PartOut::Append(s.clone())),
        WordPart::ProcessSubst(s, _) => Ok(PartOut::Append(s.clone())),
    }
}

/// Expand `$name` / `${...}`.
fn expand_parameter(ctx: &ExpandCtx, p: &Parameter, in_dquotes: bool) -> Result<PartOut, String> {
    let name = p.name.as_str();
    // Positional parameters are stored as `positional[0] = $0`, so `$1` is
    // index 1, `$@` is `positional[1..]`, `$#` is `positional.len() - 1`.
    let params = if ctx.positional.len() > 1 {
        &ctx.positional[1..]
    } else {
        &[][..]
    };
    match name {
        "?" => {
            return Ok(PartOut::Append(ctx.last_status.status_code().to_string()));
        }
        "$" => {
            return Ok(PartOut::Append(ctx.shell_pid.to_string()));
        }
        "#" => {
            return Ok(PartOut::Append(params.len().to_string()));
        }
        "@" => {
            let fields: Vec<String> = params.to_vec();
            if in_dquotes {
                // `"$@"` — one field per parameter.
                return Ok(PartOut::Fields(fields));
            }
            // Unquoted `$@`: every parameter is split like an expansion.
            let joined = fields.join(" ");
            return Ok(PartOut::Split(joined));
        }
        "*" => {
            let joined = params.join(ifs_char(ctx).to_string().as_str());
            return Ok(if in_dquotes {
                PartOut::Append(joined)
            } else {
                PartOut::Split(joined)
            });
        }
        "0" => return Ok(PartOut::Append(ctx.shell_name().to_owned())),
        _ => {}
    }

    // Positional parameters $1..$9.
    if name.len() == 1 && name.chars().next().unwrap().is_ascii_digit() {
        let idx: usize = name.parse().map_err(|_| alloc::format!("bad param `{name}`"))?;
        return Ok(match params.get(idx.saturating_sub(1)) {
            Some(v) if in_dquotes => PartOut::Append(v.clone()),
            Some(v) => PartOut::Split(v.clone()),
            None => PartOut::Nothing,
        });
    }

    // Regular variable.
    match ctx.env.get(name) {
        Some(var) => {
            let value = var.value().to_owned();
            Ok(if in_dquotes {
                PartOut::Append(value)
            } else {
                PartOut::Split(value)
            })
        }
        None => Ok(PartOut::Nothing),
    }
}

/// The first IFS character, used by `$*`.
fn ifs_char(ctx: &ExpandCtx) -> char {
    let chars = ifs_chars(ctx.env);
    *chars.first().unwrap_or(&' ')
}

/// Split `s` on IFS, folding the pieces into `fields`/`cur`.
///
/// Bash semantics: leading/trailing/consecutive delimiters are dropped. The
/// first surviving piece continues the field currently under construction;
/// later pieces start fresh fields.
fn split_fields(fields: &mut Vec<String>, cur: &mut String, s: &str, ifs: &[char]) {
    let pieces: Vec<&str> = s.split(|c| ifs.contains(&c)).collect();
    let mut start = 0;
    while start < pieces.len() && pieces[start].is_empty() {
        start += 1;
    }
    if start >= pieces.len() {
        return;
    }
    cur.push_str(pieces[start]);
    for piece in &pieces[start + 1..] {
        if !piece.is_empty() {
            fields.push(core::mem::take(cur));
            cur.push_str(piece);
        }
    }
}

/// `~`, `~/...`, `~user`. Without a real passwd lookup, `~user` falls back
/// to `$HOME` when the user matches the current user name.
fn expand_tilde(env: &EnvStack, text: &str) -> String {
    if text == "~" {
        return env.get("HOME").map(|v| v.value().to_owned()).unwrap_or_default();
    }
    let rest = &text[1..];
    let (user, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if user.is_empty() {
        // `~/...`
        let home = env.get("HOME").map(|v| v.value().to_owned()).unwrap_or_default();
        return alloc::format!("{home}{path}");
    }
    // `~user`: best-effort via $HOME when the user is the current one.
    if user == env.get("USER").map(|v| v.value().to_owned()).unwrap_or_default() {
        let home = env.get("HOME").map(|v| v.value().to_owned()).unwrap_or_default();
        return alloc::format!("{home}{path}");
    }
    // Unknown user: leave literal.
    text.to_owned()
}

/// Decode `$'...'` ANSI-C escapes.
fn ansi_c_decode(s: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            None => out.push('\\'),
            Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'),
            Some('e') | Some('E') => out.push('\x1b'),
            Some('f') => out.push('\x0c'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('v') => out.push('\x0b'),
            Some('\\') => out.push('\\'),
            Some('\'') => out.push('\''),
            Some('"') => out.push('"'),
            Some('x') | Some('u') | Some('U') => {
                // Hex escape: collect up to 4 hex digits.
                let mut hex = String::new();
                while let Some(h) = chars.next() {
                    if h.is_ascii_hexdigit() {
                        hex.push(h);
                    } else {
                        // Put the char back is hard with an iterator; we
                        // approximate by consuming it. Good enough for M2a.
                        break;
                    }
                }
                let val = u32::from_str_radix(&hex, 16).unwrap_or(0);
                if let Some(ch) = char::from_u32(val) {
                    out.push(ch);
                }
            }
            Some(c @ '0'..='7') => {
                let mut oct = String::new();
                oct.push(c);
                for _ in 0..2 {
                    if let Some(h) = chars.next() {
                        if ('0'..='7').contains(&h) {
                            oct.push(h);
                        }
                    }
                }
                let val = u32::from_str_radix(&oct, 8).unwrap_or(0);
                if let Some(ch) = char::from_u32(val) {
                    out.push(ch);
                }
            }
            Some(other) => out.push(other),
        }
    }
    Ok(out)
}

/// Helper on the expansion context for `$0`.
impl<'a> ExpandCtx<'a> {
    pub fn shell_name(&self) -> &str {
        self.positional
            .get(0)
            .map(String::as_str)
            .unwrap_or("cake")
    }
}

/// Expand the target of a redirection to a single path string (no field
/// splitting: redirection targets are a single word in bash).
pub fn expand_redirect_word(ctx: &ExpandCtx, word: &Word) -> Result<String, String> {
    expand_word_quoted(ctx, word)
}

/// Parse a plain string as a single-literal word and expand it.
/// Used by `[[ ... ]]` operand expansion.
pub fn expand_plain_string(ctx: &ExpandCtx, s: &str) -> Result<Vec<String>, String> {
    let word = cake_syntax::word::parse_word(s, cake_syntax::Span::new(0, s.len() as u32));
    expand_word(ctx, &word)
}

/// Expand parameters inside an arithmetic expression before evaluating it
/// (bash does the same: `$(( $1 + $2 ))`).
fn expand_arith_text(ctx: &ExpandCtx, s: &str) -> Result<String, String> {
    let word = cake_syntax::word::parse_word(s, cake_syntax::Span::new(0, s.len() as u32));
    let fields = expand_word(ctx, &word)?;
    Ok(fields.concat())
}

/// Build a single-literal Word from a string.
pub fn word_from_string(s: &str) -> Word {
    cake_syntax::word::parse_word(s, cake_syntax::Span::new(0, s.len() as u32))
}
