//! Word expansion.
//!
//! M2c: full bash expansion order — brace expansion, tilde, parameter
//! expansion (`$var`, `${var}`, operators), command substitution, arithmetic
//! expansion, IFS field splitting, pathname (glob) expansion, quote removal.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use cake_env::EnvStack;
use cake_proc::ProcStatus;
use cake_syntax::{Command, Parameter, Word, WordPart};

/// Context for expanding a word. Holds the environment mutably so that
/// `${x:=default}` can assign back into it.
pub struct ExpandCtx<'a> {
    pub env: &'a mut EnvStack,
    pub last_status: ProcStatus,
    /// Positional parameters (`$1`, `$@`, ...).
    pub positional: &'a [String],
    /// Defined functions (for command substitution sub-shells).
    pub functions: &'a BTreeMap<String, Command>,
    /// Defined aliases (for command substitution sub-shells).
    pub aliases: &'a BTreeMap<String, String>,
    /// The shell's own pid for `$$`.
    pub shell_pid: i32,
    /// `set -u`: an unset variable is an error.
    pub nounset: bool,
    /// `set -f`: skip globbing.
    pub noglob: bool,
    /// `shopt` toggles for globbing.
    pub shopt: crate::executor::ShoptBits,
    /// `set -e`: propagate to command-substitution sub-shells.
    pub errexit: bool,
    /// Pid of the most recent background job (`$!`).
    pub last_bg_pid: i32,
    /// LCG state for `$RANDOM` (mutated on each expansion).
    pub random_state: &'a mut u32,
    /// Shell start time (epoch seconds) for `$SECONDS`.
    pub start_time: i64,
    /// Current line number (`$LINENO`; one-per-command approximation).
    pub lineno: u32,
    /// Parent process id (`$PPID`).
    pub parent_pid: i32,
    /// Fds backing `<(cmd)`/`>(cmd)` process substitutions; kept open until
    /// the end of evaluation.
    pub proc_subst_fds: &'a mut Vec<cake_platform::Fd>,
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
/// Follows bash's order: brace expansion first (may yield several words),
/// then per word: tilde/parameter/command/arithmetic expansion, IFS field
/// splitting, then pathname (glob) expansion of the resulting fields. Empty
/// results are dropped, so `""` / `$unset` in a word can yield zero fields.
pub fn expand_word(ctx: &mut ExpandCtx, word: &Word) -> Result<Vec<String>, String> {
    let mut all = Vec::new();
    for sub in brace_expand(word)? {
        all.extend(expand_fields(ctx, &sub, false, true)?);
    }
    Ok(all)
}

/// The part loop: expand each part, apply IFS splitting, then (when `do_glob`)
/// glob the unquoted fields. Used by [`expand_word`] (argv, with globbing),
/// and with `do_glob == false` for operands and arithmetic text.
fn expand_fields(
    ctx: &mut ExpandCtx,
    word: &Word,
    in_dquotes: bool,
    do_glob: bool,
) -> Result<Vec<String>, String> {
    let ifs = ifs_chars(ctx.env);
    let mut fields: Vec<String> = Vec::new();
    // Glob eligibility per field: only unquoted literal text can glob.
    let mut glob_ok: Vec<bool> = Vec::new();
    let mut cur = String::new();
    let mut cur_glob = false;
    // A quoted empty (`""`) yields one empty field rather than nothing.
    let mut cur_quoted = false;

    for part in &word.parts {
        let eligible = matches!(part, WordPart::Literal(..));
        let quoted = matches!(
            part,
            WordPart::SingleQuoted(..) | WordPart::DoubleQuoted(..) | WordPart::AnsiCQuoted(..)
        );
        match expand_part(ctx, part, in_dquotes)? {
            PartOut::Append(s) => {
                cur.push_str(&s);
                if eligible && crate::glob::has_glob_chars(&s) {
                    cur_glob = true;
                }
                if quoted && s.is_empty() {
                    cur_quoted = true;
                }
            }
            PartOut::Split(s) => {
                split_fields(
                    &mut fields,
                    &mut glob_ok,
                    &mut cur,
                    &mut cur_glob,
                    &mut cur_quoted,
                    &s,
                    &ifs,
                );
            }
            PartOut::Fields(v) => {
                if !cur.is_empty() || !v.is_empty() {
                    if !cur.is_empty() {
                        fields.push(core::mem::take(&mut cur));
                        glob_ok.push(core::mem::take(&mut cur_glob));
                        cur_quoted = false;
                    }
                    for f in v {
                        fields.push(f);
                        glob_ok.push(false);
                    }
                }
            }
            PartOut::Nothing => {}
        }
    }
    if !cur.is_empty() || cur_quoted {
        fields.push(core::mem::take(&mut cur));
        glob_ok.push(core::mem::take(&mut cur_glob));
    }

    if in_dquotes || !do_glob || ctx.noglob {
        return Ok(fields);
    }
    // Pathname expansion: each unquoted field containing glob metachars is
    // matched against the filesystem. No match → the literal pattern stays,
    // unless `nullglob` removes the field entirely.
    let mut out = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        if glob_ok[i] && crate::glob::has_glob_chars(f) {
            let matches = crate::glob::expand_glob(f, ctx.shopt.dotglob, ctx.shopt.nocaseglob);
            if !matches.is_empty() {
                out.extend(matches);
                continue;
            }
            if ctx.shopt.nullglob {
                continue;
            }
        }
        out.push(f.clone());
    }
    Ok(out)
}

/// Expand a word inside double quotes: the result is always a single field
/// (no splitting), except for `$@` which expands to one field per positional
/// parameter.
pub fn expand_word_quoted(ctx: &mut ExpandCtx, word: &Word) -> Result<String, String> {
    let mut out = String::new();
    for part in &word.parts {
        match expand_part(ctx, part, true)? {
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

/// Brace expansion: turn a word containing `{a,b}` / `{1..5}` into several
/// words (Cartesian product with the surrounding literal parts).
///
/// Only top-level `Brace` parts expand; braces inside quotes (nested in a
/// `DoubleQuoted` part) are literal and never become `Brace` parts.
fn brace_expand(word: &Word) -> Result<Vec<Word>, String> {
    let mut words: Vec<Vec<WordPart>> = vec![Vec::new()];
    for part in &word.parts {
        if let WordPart::Brace(text, _) = part {
            let alts = brace_alternatives(text)?;
            let mut next: Vec<Vec<WordPart>> = Vec::new();
            for w in &words {
                for a in &alts {
                    let mut w2 = w.clone();
                    w2.push(WordPart::Literal(a.clone(), word.span));
                    next.push(w2);
                }
            }
            words = next;
        } else {
            for w in &mut words {
                w.push(part.clone());
            }
        }
    }
    Ok(words
        .into_iter()
        .map(|parts| Word {
            parts,
            span: word.span,
        })
        .collect())
}

/// The alternatives of one `{...}` brace (text includes the braces).
fn brace_alternatives(text: &str) -> Result<Vec<String>, String> {
    let inner = &text[1..text.len() - 1];
    if inner.contains(',') {
        // Comma list (no nested expansion for now).
        return Ok(split_top_commas(inner));
    }
    if let Some((lo, hi, step)) = parse_range(inner)
        && let Some(vals) = expand_range(&lo, &hi, step)
    {
        return Ok(vals);
    }
    // `{a}` or unexpandable: bash keeps the braces literal.
    Ok(vec![text.to_owned()])
}

/// Split on commas that are not inside nested `{...}`.
fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '{' => {
                depth += 1;
                cur.push(c);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(core::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn parse_range(inner: &str) -> Option<(String, String, i64)> {
    let mut parts = inner.split("..");
    let lo = parts.next()?.trim();
    let hi = parts.next()?.trim();
    let step = parts
        .next()
        .map(|s| s.trim().parse::<i64>().unwrap_or(1))
        .unwrap_or(1);
    if lo.is_empty() || hi.is_empty() {
        return None;
    }
    Some((lo.to_owned(), hi.to_owned(), step))
}

fn expand_range(lo: &str, hi: &str, step: i64) -> Option<Vec<String>> {
    if let (Ok(l), Ok(h)) = (lo.parse::<i64>(), hi.parse::<i64>()) {
        let width = lo.len().max(hi.len());
        let mut out = Vec::new();
        let st = step.abs().max(1);
        if l <= h {
            let mut n = l;
            while n <= h {
                out.push(zpad(n, width));
                n += st;
            }
        } else {
            let mut n = l;
            while n >= h {
                out.push(zpad(n, width));
                n = n.saturating_sub(st);
            }
        }
        return Some(out);
    }
    if lo.len() == 1 && hi.len() == 1 {
        let (a, b) = (lo.chars().next()?, hi.chars().next()?);
        if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() {
            let mut out = Vec::new();
            if a <= b {
                for c in (a as u8)..=(b as u8) {
                    out.push((c as char).to_string());
                }
            } else {
                for c in (b as u8)..=(a as u8) {
                    out.push((c as char).to_string());
                }
            }
            return Some(out);
        }
    }
    None
}

fn zpad(n: i64, width: usize) -> String {
    let s = n.to_string();
    if width > s.len() {
        alloc::format!("{:0>width$}", s)
    } else {
        s
    }
}

fn expand_part(ctx: &mut ExpandCtx, part: &WordPart, in_dquotes: bool) -> Result<PartOut, String> {
    match part {
        WordPart::Literal(s, _) => Ok(PartOut::Append(s.clone())),
        WordPart::SingleQuoted(s, _) => Ok(PartOut::Append(s.clone())),
        WordPart::AnsiCQuoted(s, _) => Ok(PartOut::Append(ansi_c_decode(s)?)),
        WordPart::DoubleQuoted(parts, _) => {
            let mut out = String::new();
            for p in parts {
                match expand_part(ctx, p, true)? {
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
        // Command substitution `$(...)` / backticks: run the body in a child
        // and capture its stdout.
        WordPart::CommandSubst(s, _) => expand_command_subst(ctx, s, in_dquotes),
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
        // Brace expansion is handled as a pre-pass in `expand_word`; a
        // `Brace` part that survives (quoted) is literal.
        WordPart::Brace(s, _) => Ok(PartOut::Append(s.clone())),
        WordPart::ProcessSubst(s, _) => expand_process_subst(ctx, s),
    }
}

/// Evaluate `cmd` in a forked child with stdout captured, then strip all
/// trailing newlines (bash semantics).
/// `<(cmd)` / `>(cmd)`: run `cmd` in a child connected to a pipe, expand to
/// the pipe end's `/dev/fd/N` path. The fd is kept open (registered on the
/// executor) until evaluation finishes, so the command can read/write it.
fn expand_process_subst(ctx: &mut ExpandCtx, raw: &str) -> Result<PartOut, String> {
    let (is_input, body) = match raw.as_bytes().first() {
        Some(b'<') => (true, &raw[2..raw.len().saturating_sub(1)]),
        Some(b'>') => (false, &raw[2..raw.len().saturating_sub(1)]),
        _ => return Err(alloc::format!("cake: bad process substitution `{raw}`")),
    };
    let p = cake_platform::get();
    let (r, w) = p
        .pipe(false)
        .map_err(|e| alloc::format!("cake: pipe: {e}"))?;
    let mut env = Some(ctx.env.clone());
    let mut positional = Some(ctx.positional.to_vec());
    let mut functions = Some(ctx.functions.clone());
    let mut aliases = Some(ctx.aliases.clone());
    let nounset = ctx.nounset;
    let noglob = ctx.noglob;
    let shopt = ctx.shopt;
    let errexit = ctx.errexit;
    let handle = p.run_in_child(&mut move || {
        let mut sub = crate::executor::Executor::new(env.take().unwrap_or_default());
        sub.positional = positional.take().unwrap_or_default();
        sub.functions = functions.take().unwrap_or_default();
        sub.aliases = aliases.take().unwrap_or_default();
        sub.nounset = nounset;
        sub.noglob = noglob;
        sub.shopt = shopt;
        sub.errexit = errexit;
        if is_input {
            // `<(cmd)`: the command's stdout feeds the pipe.
            let _ = p.dup2(w, 1);
            let _ = p.close(r);
        } else {
            // `>(cmd)`: the command reads the pipe on stdin.
            let _ = p.dup2(r, 0);
            let _ = p.close(w);
        }
        let outcome = sub.eval_str(body);
        outcome.status.status_code()
    });
    let keep = if is_input {
        let _ = p.close(w);
        r
    } else {
        let _ = p.close(r);
        w
    };
    let handle = handle.map_err(|e| {
        let _ = p.close(keep);
        alloc::format!("cake: process substitution: {e}")
    })?;
    // Reap asynchronously: the child may outlive the command that consumed
    // the fd (bash waits for it when the shell exits).
    let _ = p.wait(&handle, cake_platform::WaitOptions::NOHANG);
    ctx.proc_subst_fds.push(keep);
    Ok(PartOut::Append(p.fd_path(keep)))
}

/// Evaluate `cmd` in a forked child with stdout captured, then strip all
/// trailing newlines (bash semantics).
fn expand_command_subst(ctx: &mut ExpandCtx, cmd: &str, in_dquotes: bool) -> Result<PartOut, String> {
    let p = cake_platform::get();
    let (r, w) = p
        .pipe(false)
        .map_err(|e| alloc::format!("cake: pipe: {e}"))?;
    // Snapshot the state the sub-shell needs (env, positionals, functions,
    // aliases) so the child can build its own Executor without sharing ours.
    let mut env = Some(ctx.env.clone());
    let mut positional = Some(ctx.positional.to_vec());
    let mut functions = Some(ctx.functions.clone());
    let mut aliases = Some(ctx.aliases.clone());
    let nounset = ctx.nounset;
    let noglob = ctx.noglob;
    let shopt = ctx.shopt;
    let errexit = ctx.errexit;
    let handle = p.run_in_child(&mut move || {
        let _ = p.dup2(w, 1);
        let _ = p.close(r);
        let mut sub = crate::executor::Executor::new(env.take().unwrap_or_default());
        sub.positional = positional.take().unwrap_or_default();
        sub.functions = functions.take().unwrap_or_default();
        sub.aliases = aliases.take().unwrap_or_default();
        sub.nounset = nounset;
        sub.noglob = noglob;
        sub.shopt = shopt;
        sub.errexit = errexit;
        let outcome = sub.eval_str(cmd);
        outcome.status.status_code()
    });
    let _ = p.close(w);
    let handle = handle.map_err(|e| {
        let _ = p.close(r);
        alloc::format!("cake: $(...): {e}")
    })?;

    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match p.read(r, &mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let _ = p.close(r);
    let _ = p.wait(&handle, cake_platform::WaitOptions::NONE);

    let text = String::from_utf8_lossy(&out);
    let trimmed = text.trim_end_matches('\n');
    Ok(if in_dquotes {
        PartOut::Append(trimmed.to_owned())
    } else if trimmed.is_empty() {
        PartOut::Nothing
    } else {
        PartOut::Split(trimmed.to_owned())
    })
}

/// `$RANDOM`: LCG (numerical recipes), 15-bit like bash.
fn next_random(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    (*state >> 16) & 0x7fff
}

/// Expand `$name` / `${...}`.
fn expand_parameter(ctx: &mut ExpandCtx, p: &Parameter, in_dquotes: bool) -> Result<PartOut, String> {
    let (name, index, op) = parse_param(&p.text);
    if !matches!(op, ParamOp::Normal) {
        return apply_param_op(ctx, &name, index, op, in_dquotes);
    }
    // Normal expansion with an index → array element access.
    if let Some(idx) = &index {
        return expand_indexed(ctx, &name, idx, in_dquotes);
    }
    let name = name.as_str();
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
        "!" => {
            return Ok(PartOut::Append(ctx.last_bg_pid.to_string()));
        }
        "RANDOM" => {
            let v = next_random(ctx.random_state);
            return Ok(PartOut::Append(v.to_string()));
        }
        "SECONDS" => {
            let now = cake_platform::get().time_seconds();
            return Ok(PartOut::Append((now - ctx.start_time).max(0).to_string()));
        }
        "LINENO" => {
            return Ok(PartOut::Append(ctx.lineno.to_string()));
        }
        "PPID" => {
            return Ok(PartOut::Append(ctx.parent_pid.to_string()));
        }
        _ => {}
    }

    // Positional parameters $1..$9, and ${10}... in the braced form.
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
        let idx: usize = name.parse().map_err(|_| alloc::format!("bad param `{name}`"))?;
        return Ok(match params.get(idx.saturating_sub(1)) {
            Some(v) if in_dquotes => PartOut::Append(v.clone()),
            Some(v) => PartOut::Split(v.clone()),
            None => {
                if ctx.nounset {
                    return Err(alloc::format!("cake: ${name}: unbound variable"));
                }
                PartOut::Nothing
            }
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
        None => {
            if ctx.nounset {
                return Err(alloc::format!("cake: ${name}: unbound variable"));
            }
            Ok(PartOut::Nothing)
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter operators: `${x:-d}`, `${#x}`, `${x:0:5}`, `${x/pat/rep}`, ...
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum ParamOp {
    Normal,
    /// `${#x}` — length.
    Length,
    /// `${x:off:len}` — substring (raw spec after the `:`).
    Substring(String),
    /// `${x-word}` / `${x:-word}` — default (colon = also when null).
    Default(String, bool),
    /// `${x=word}` / `${x:=word}` — assign if unset (colon = also when null).
    Assign(String, bool),
    /// `${x+word}` / `${x:+word}` — alternate value (colon = only when non-null).
    Alt(String, bool),
    /// `${x?word}` / `${x:?word}` — error if unset (colon = also when null).
    Error(String, bool),
    /// `${x#pat}` / `${x##pat}` — remove prefix (bool = longest).
    RemovePrefix(String, bool),
    /// `${x%pat}` / `${x%%pat}` — remove suffix (bool = longest).
    RemoveSuffix(String, bool),
    /// `${x/pat/repl}` — replace.
    Replace(String, String, ReplaceKind),
    /// `${x^}` / `${x^^}` — uppercase (bool = all).
    Upper(bool),
    /// `${x,}` / `${x,,}` — lowercase (bool = all).
    Lower(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplaceKind {
    /// `/` — replace the longest match anywhere.
    First,
    /// `//` — replace all non-overlapping matches.
    All,
    /// `/#` — anchored at the start.
    Start,
    /// `/%` — anchored at the end.
    End,
}

/// Parse a parameter expansion's raw text (`$name` or `${...}`) into a name,
/// an optional index (`arr[0]`, `arr[@]`), and an operator.
fn parse_param(text: &str) -> (String, Option<String>, ParamOp) {
    if !text.starts_with("${") {
        return (text[1..].to_owned(), None, ParamOp::Normal);
    }
    let inner = &text[2..text.len() - 1];
    // `${#name}` is length; a `#` after the name is prefix removal.
    if let Some(rest) = inner.strip_prefix('#')
        && !rest.is_empty()
    {
        return (base_name(rest).to_owned(), split_index(rest), ParamOp::Length);
    }
    const OP_CHARS: [char; 11] = [':', '#', '%', '/', '=', '+', '?', '-', '^', ',', '['];
    let name_end = inner.find(OP_CHARS).unwrap_or(inner.len());
    let name = &inner[..name_end];
    let rest = &inner[name_end..];
    // Split an optional `[index]` from the operator.
    let (name, index, op) = if let Some(stripped) = rest.strip_prefix('[') {
        if let Some((idx, after)) = stripped.split_once(']') {
            (name, Some(idx.to_owned()), after)
        } else {
            (name, None, rest)
        }
    } else {
        (name, None, rest)
    };
    let op = parse_op(op);
    (name.to_owned(), index, op)
}

/// Strip `[index]` from the end of a name (for `${#arr[@]}`).
fn split_index(inner: &str) -> Option<String> {
    if let Some(i) = inner.rfind('[')
        && inner[i..].ends_with(']')
    {
        let idx = &inner[i + 1..inner.len() - 1];
        return Some(idx.to_owned());
    }
    None
}

/// The variable name portion of `${...}` (everything up to an operator char).
fn base_name(inner: &str) -> &str {
    const OP_CHARS: [char; 11] = [':', '#', '%', '/', '=', '+', '?', '-', '^', ',', '['];
    inner.find(OP_CHARS).map_or(inner, |i| &inner[..i])
}

fn parse_op(op: &str) -> ParamOp {
    match op.chars().next() {
        None => ParamOp::Normal,
        Some(':') => match op.chars().nth(1) {
            Some('-') => ParamOp::Default(op[2..].to_owned(), true),
            Some('=') => ParamOp::Assign(op[2..].to_owned(), true),
            Some('+') => ParamOp::Alt(op[2..].to_owned(), true),
            Some('?') => ParamOp::Error(op[2..].to_owned(), true),
            _ => ParamOp::Substring(op[1..].to_owned()),
        },
        Some('-') => ParamOp::Default(op[1..].to_owned(), false),
        Some('=') => ParamOp::Assign(op[1..].to_owned(), false),
        Some('+') => ParamOp::Alt(op[1..].to_owned(), false),
        Some('?') => ParamOp::Error(op[1..].to_owned(), false),
        Some('#') if op.starts_with("##") => ParamOp::RemovePrefix(op[2..].to_owned(), true),
        Some('#') => ParamOp::RemovePrefix(op[1..].to_owned(), false),
        Some('%') if op.starts_with("%%") => ParamOp::RemoveSuffix(op[2..].to_owned(), true),
        Some('%') => ParamOp::RemoveSuffix(op[1..].to_owned(), false),
        Some('/') => {
            let (kind, rest) = if let Some(rest) = op.strip_prefix("//") {
                (ReplaceKind::All, rest)
            } else if let Some(rest) = op.strip_prefix("/#") {
                (ReplaceKind::Start, rest)
            } else if let Some(rest) = op.strip_prefix("/%") {
                (ReplaceKind::End, rest)
            } else {
                (ReplaceKind::First, &op[1..])
            };
            let (pat, repl) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i + 1..]),
                None => (rest, ""),
            };
            ParamOp::Replace(pat.to_owned(), repl.to_owned(), kind)
        }
        Some('^') => ParamOp::Upper(op.starts_with("^^")),
        Some(',') => ParamOp::Lower(op.starts_with(",,")),
        _ => ParamOp::Normal,
    }
}

/// The value of `name`, or `None` if unset. Multi-valued special params are
/// joined for operator purposes.
fn param_value(ctx: &ExpandCtx, name: &str) -> Option<String> {
    let params = if ctx.positional.len() > 1 {
        &ctx.positional[1..]
    } else {
        &[][..]
    };
    match name {
        "@" => Some(params.join(" ")),
        "*" => Some(params.join(ifs_char(ctx).to_string().as_str())),
        "?" => Some(ctx.last_status.status_code().to_string()),
        "$" => Some(ctx.shell_pid.to_string()),
        "#" => Some(params.len().to_string()),
        "0" => Some(ctx.shell_name().to_owned()),
        _ if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) => {
            let idx: usize = name.parse().unwrap_or(1);
            params.get(idx.saturating_sub(1)).cloned()
        }
        _ => ctx.env.get(name).map(|v| v.value().to_owned()),
    }
}

/// Wrap a value into a `PartOut` (subject to IFS splitting when unquoted).
fn part_value(value: &Option<String>, in_dquotes: bool) -> PartOut {
    match value {
        Some(v) if in_dquotes => PartOut::Append(v.clone()),
        Some(v) => PartOut::Split(v.clone()),
        None => PartOut::Nothing,
    }
}

/// The values of an array variable (or positional params for `@`/`*`).
fn array_values(ctx: &ExpandCtx, name: &str) -> Vec<String> {
    let params = if ctx.positional.len() > 1 {
        &ctx.positional[1..]
    } else {
        &[][..]
    };
    match name {
        "@" | "*" => params.to_vec(),
        _ => ctx.env.get(name).map(|v| v.values().to_vec()).unwrap_or_default(),
    }
}

/// Expand `${arr[idx]}` (Normal op with index). `@`/`*` → all elements.
fn expand_indexed(ctx: &mut ExpandCtx, name: &str, idx: &str, in_dquotes: bool) -> Result<PartOut, String> {
    let values = array_values(ctx, name);
    if idx == "@" || idx == "*" {
        if in_dquotes && idx == "@" {
            return Ok(PartOut::Fields(values));
        }
        let joined = values.join(" ");
        return Ok(if in_dquotes {
            PartOut::Append(joined)
        } else {
            PartOut::Split(joined)
        });
    }
    let n = crate::arith::eval_arith_value(ctx.env, idx)
        .unwrap_or_default()
        .parse::<i64>()
        .unwrap_or(0) as usize;
    match values.get(n) {
        Some(v) => Ok(if in_dquotes {
            PartOut::Append(v.clone())
        } else {
            PartOut::Split(v.clone())
        }),
        None => {
            if ctx.nounset {
                return Err(alloc::format!("cake: {name}[{idx}]: unbound variable"));
            }
            Ok(PartOut::Nothing)
        }
    }
}

fn apply_param_op(ctx: &mut ExpandCtx, name: &str, index: Option<String>, op: ParamOp, in_dquotes: bool) -> Result<PartOut, String> {
    // Resolve the value (single string) for the name/index.
    let value: Option<String> = match &index {
        Some(idx) if idx == "@" || idx == "*" => None,  // handled specially
        Some(idx) => {
            let values = array_values(ctx, name);
            let n = crate::arith::eval_arith_value(ctx.env, idx)
                .unwrap_or_default()
                .parse::<i64>()
                .unwrap_or(0) as usize;
            values.get(n).cloned()
        }
        None => param_value(ctx, name),
    };
    match op {
        ParamOp::Length => {
            if matches!(index.as_deref(), Some("@") | Some("*")) {
                Ok(PartOut::Append(array_values(ctx, name).len().to_string()))
            } else {
                if value.is_none() && ctx.nounset {
                    return Err(alloc::format!("cake: ${name}: unbound variable"));
                }
                let len = value.as_ref().map(|v| v.chars().count()).unwrap_or(0);
                Ok(PartOut::Append(len.to_string()))
            }
        }
        ParamOp::Substring(spec) => {
            let v = value.unwrap_or_default();
            let (off, len) = parse_substring(&spec);
            let n = v.chars().count() as i64;
            let start = if off < 0 { (n + off).max(0) } else { off.min(n) };
            let end = match len {
                Some(l) if l < 0 => (n + l).max(start) as usize,
                Some(l) => ((start + l).min(n)).max(0) as usize,
                None => n as usize,
            };
            let s: String = v.chars().skip(start as usize).take(end.saturating_sub(start as usize)).collect();
            Ok(part_value(&Some(s), in_dquotes))
        }
        ParamOp::Default(word, colon) => {
            let use_default = match &value {
                None => true,
                Some(v) => colon && v.is_empty(),
            };
            if use_default {
                let w = expand_operand(ctx, &word)?;
                Ok(part_value(&Some(w), in_dquotes))
            } else {
                Ok(part_value(&value, in_dquotes))
            }
        }
        ParamOp::Assign(word, colon) => {
            let use_default = match &value {
                None => true,
                Some(v) => colon && v.is_empty(),
            };
            if use_default {
                let w = expand_operand(ctx, &word)?;
                let _ = ctx.env.set(name, cake_env::EnvVar::new(w.clone()));
                Ok(part_value(&Some(w), in_dquotes))
            } else {
                Ok(part_value(&value, in_dquotes))
            }
        }
        ParamOp::Alt(word, colon) => {
            let use_alt = match &value {
                None => false,
                Some(v) => !(colon && v.is_empty()),
            };
            if use_alt {
                let w = expand_operand(ctx, &word)?;
                Ok(part_value(&Some(w), in_dquotes))
            } else {
                Ok(PartOut::Nothing)
            }
        }
        ParamOp::Error(word, colon) => {
            let unset_or_null = match &value {
                None => true,
                Some(v) => colon && v.is_empty(),
            };
            if unset_or_null {
                let msg = expand_operand(ctx, &word)?;
                Err(alloc::format!("cake: {name}: {msg}"))
            } else {
                Ok(part_value(&value, in_dquotes))
            }
        }
        ParamOp::RemovePrefix(pat, longest) => {
            let v = value.unwrap_or_default();
            let pat = expand_operand(ctx, &pat)?;
            Ok(part_value(&Some(trim_prefix(&v, &pat, longest)), in_dquotes))
        }
        ParamOp::RemoveSuffix(pat, longest) => {
            let v = value.unwrap_or_default();
            let pat = expand_operand(ctx, &pat)?;
            Ok(part_value(&Some(trim_suffix(&v, &pat, longest)), in_dquotes))
        }
        ParamOp::Replace(pat, repl, kind) => {
            let v = value.unwrap_or_default();
            let pat = expand_operand(ctx, &pat)?;
            let repl = expand_operand(ctx, &repl)?;
            Ok(part_value(&Some(replace_glob(&v, &pat, &repl, kind)), in_dquotes))
        }
        ParamOp::Upper(all) => {
            let v = value.unwrap_or_default();
            let out = if all {
                v.to_uppercase()
            } else {
                let mut c = v.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => v,
                }
            };
            Ok(part_value(&Some(out), in_dquotes))
        }
        ParamOp::Lower(all) => {
            let v = value.unwrap_or_default();
            let out = if all {
                v.to_lowercase()
            } else {
                let mut c = v.chars();
                match c.next() {
                    Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
                    None => v,
                }
            };
            Ok(part_value(&Some(out), in_dquotes))
        }
        ParamOp::Normal => unreachable!(),
    }
}

/// Expand an operator operand (`:-word`, `#pat`, ...) to a single string.
fn expand_operand(ctx: &mut ExpandCtx, text: &str) -> Result<String, String> {
    if text.is_empty() {
        return Ok(String::new());
    }
    let fields = expand_plain_string(ctx, text)?;
    Ok(fields.join(" "))
}

/// Parse `${x:off:len}` into (offset, length). Offsets may be negative.
fn parse_substring(spec: &str) -> (i64, Option<i64>) {
    let spec = spec.trim();
    let mut parts = spec.splitn(2, ':');
    let off = parts.next().unwrap_or("").trim().parse::<i64>().unwrap_or(0);
    let len = parts.next().map(|s| s.trim().parse::<i64>().unwrap_or(0));
    (off, len)
}

/// Character-boundary byte offsets of `s` (start and end of every char).
fn char_offsets(s: &str) -> Vec<usize> {
    let mut out = Vec::new();
    out.push(0);
    out.extend(s.char_indices().map(|(i, c)| i + c.len_utf8()));
    out
}

/// Remove the shortest/longest prefix of `s` matching glob `pat`.
fn trim_prefix(s: &str, pat: &str, longest: bool) -> String {
    let offsets = char_offsets(s);
    let range: Vec<usize> = if longest {
        offsets.iter().copied().rev().collect()
    } else {
        offsets.clone()
    };
    for end in range {
        if end > 0 && crate::glob::glob_match(pat, &s[..end]) {
            return s[end..].to_owned();
        }
    }
    s.to_owned()
}

/// Remove the shortest/longest suffix of `s` matching glob `pat`.
fn trim_suffix(s: &str, pat: &str, longest: bool) -> String {
    let offsets = char_offsets(s);
    // A suffix is `s[start..]`. The longest suffix starts earliest
    // (ascending); the shortest starts latest (descending).
    let range: Vec<usize> = if longest {
        offsets.clone()
    } else {
        offsets.iter().copied().rev().collect()
    };
    for start in range {
        if start < s.len() && crate::glob::glob_match(pat, &s[start..]) {
            return s[..start].to_owned();
        }
    }
    s.to_owned()
}

/// The longest substring of `s` starting at `start` that matches `pat`.
fn match_at(s: &str, pat: &str, start: usize) -> Option<usize> {
    let offsets = char_offsets(s);
    for end in offsets.iter().copied().rev() {
        if end > start && crate::glob::glob_match(pat, &s[start..end]) {
            return Some(end);
        }
    }
    None
}

fn replace_glob(s: &str, pat: &str, repl: &str, kind: ReplaceKind) -> String {
    if pat.is_empty() {
        return s.to_owned();
    }
    match kind {
        ReplaceKind::Start => match match_at(s, pat, 0) {
            Some(end) => alloc::format!("{repl}{}", &s[end..]),
            None => s.to_owned(),
        },
        ReplaceKind::End => {
            let offsets = char_offsets(s);
            for start in offsets.iter().copied().rev() {
                if start < s.len() && crate::glob::glob_match(pat, &s[start..]) {
                    return alloc::format!("{}{repl}", &s[..start]);
                }
            }
            s.to_owned()
        }
        ReplaceKind::First => {
            let mut i = 0;
            let offsets = char_offsets(s);
            while i < s.len() {
                if let Some(end) = match_at(s, pat, i) {
                    return alloc::format!("{}{repl}{}", &s[..i], &s[end..]);
                }
                // advance to next char boundary
                i = offsets.iter().copied().find(|&o| o > i).unwrap_or(s.len());
            }
            s.to_owned()
        }
        ReplaceKind::All => {
            let mut out = String::new();
            let mut i = 0;
            let offsets = char_offsets(s);
            while i < s.len() {
                match match_at(s, pat, i) {
                    Some(end) => {
                        out.push_str(repl);
                        i = end;
                    }
                    None => {
                        // copy one char
                        let next = offsets.iter().copied().find(|&o| o > i).unwrap_or(s.len());
                        out.push_str(&s[i..next]);
                        i = next;
                    }
                }
            }
            out.push_str(&s[i..]);
            out
        }
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
/// later pieces start fresh fields (which are never glob-eligible, since the
/// split content came from an expansion).
fn split_fields(
    fields: &mut Vec<String>,
    glob_ok: &mut Vec<bool>,
    cur: &mut String,
    cur_glob: &mut bool,
    cur_quoted: &mut bool,
    s: &str,
    ifs: &[char],
) {
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
            glob_ok.push(core::mem::take(cur_glob));
            *cur_quoted = false;
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
                for h in chars.by_ref() {
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
                    if let Some(h) = chars.next()
                        && ('0'..='7').contains(&h)
                    {
                        oct.push(h);
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
            .first()
            .map(String::as_str)
            .unwrap_or("cake")
    }
}

/// Expand the target of a redirection to a single path string (no field
/// splitting: redirection targets are a single word in bash).
pub fn expand_redirect_word(ctx: &mut ExpandCtx, word: &Word) -> Result<String, String> {
    expand_word_quoted(ctx, word)
}

/// Parse a plain string as a single-literal word and expand it (no pathname
/// expansion). Used by `[[ ... ]]` operand expansion and operator operands.
pub fn expand_plain_string(ctx: &mut ExpandCtx, s: &str) -> Result<Vec<String>, String> {
    let word = cake_syntax::word::parse_word(s, cake_syntax::Span::new(0, s.len() as u32));
    expand_fields(ctx, &word, false, false)
}

/// Expand parameters inside an arithmetic expression before evaluating it
/// (bash does the same: `$(( $1 + $2 ))`). No globbing.
fn expand_arith_text(ctx: &mut ExpandCtx, s: &str) -> Result<String, String> {
    let word = cake_syntax::word::parse_word(s, cake_syntax::Span::new(0, s.len() as u32));
    let fields = expand_fields(ctx, &word, false, false)?;
    Ok(fields.concat())
}

/// Build a single-literal Word from a string.
pub fn word_from_string(s: &str) -> Word {
    cake_syntax::word::parse_word(s, cake_syntax::Span::new(0, s.len() as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cake_env::{EnvStack, EnvVar};

    fn expand_str(env: &mut EnvStack, s: &str) -> Vec<String> {
        let word = cake_syntax::word::parse_word(s, cake_syntax::Span::new(0, s.len() as u32));
        let mut ctx = ExpandCtx {
            env,
            last_status: ProcStatus::Exit(0),
            positional: &[],
            functions: &BTreeMap::new(),
            aliases: &BTreeMap::new(),
            shell_pid: 123,
            nounset: false,
            errexit: false,
            noglob: false,
            shopt: crate::executor::ShoptBits::default(),
            last_bg_pid: 0,
            random_state: &mut 0,
            start_time: 0,
            lineno: 1,
            parent_pid: 0,
            proc_subst_fds: &mut alloc::vec::Vec::new(),
        };
        expand_word(&mut ctx, &word).unwrap()
    }

    #[test]
    fn brace_comma_list() {
        let mut env = EnvStack::new();
        assert_eq!(
            expand_str(&mut env, "file_{1,2,3}.txt"),
            ["file_1.txt", "file_2.txt", "file_3.txt"]
        );
    }

    #[test]
    fn brace_ranges() {
        let mut env = EnvStack::new();
        assert_eq!(expand_str(&mut env, "{a..c}"), ["a", "b", "c"]);
        assert_eq!(expand_str(&mut env, "{1..5..2}"), ["1", "3", "5"]);
        assert_eq!(expand_str(&mut env, "{01..03}"), ["01", "02", "03"]);
        assert_eq!(expand_str(&mut env, "{3..1}"), ["3", "2", "1"]);
    }

    #[test]
    fn brace_unexpandable_stays_literal() {
        let mut env = EnvStack::new();
        assert_eq!(expand_str(&mut env, "x{a}"), ["x{a}"]);
    }

    #[test]
    fn param_length() {
        let mut env = EnvStack::new();
        env.set("x", EnvVar::new("hello")).unwrap();
        assert_eq!(expand_str(&mut env, "${#x}"), ["5"]);
    }

    #[test]
    fn param_substring() {
        let mut env = EnvStack::new();
        env.set("x", EnvVar::new("hello world")).unwrap();
        assert_eq!(expand_str(&mut env, "${x:0:5}"), ["hello"]);
        assert_eq!(expand_str(&mut env, "${x:6}"), ["world"]);
    }

    #[test]
    fn param_replace() {
        let mut env = EnvStack::new();
        env.set("x", EnvVar::new("hello world")).unwrap();
        // Unquoted expansion is subject to IFS splitting.
        assert_eq!(expand_str(&mut env, "${x/world/there}"), ["hello", "there"]);
        env.set("y", EnvVar::new("a-b-b-c")).unwrap();
        assert_eq!(expand_str(&mut env, "${y//b/X}"), ["a-X-X-c"]);
        assert_eq!(expand_str(&mut env, "${y/#a/Z}"), ["Z-b-b-c"]);
        assert_eq!(expand_str(&mut env, "${y/%c/Z}"), ["a-b-b-Z"]);
    }

    #[test]
    fn param_default() {
        let mut env = EnvStack::new();
        assert_eq!(expand_str(&mut env, "${unset:-default}"), ["default"]);
        env.set("x", EnvVar::new("value")).unwrap();
        assert_eq!(expand_str(&mut env, "${x-default}"), ["value"]);
        // null: `-` keeps it (empty → no field), `:-` uses default.
        env.set("n", EnvVar::new("")).unwrap();
        assert_eq!(expand_str(&mut env, "${n-default}"), Vec::<String>::new());
        assert_eq!(expand_str(&mut env, "${n:-default}"), ["default"]);
    }

    #[test]
    fn param_assign_writes_env() {
        let mut env = EnvStack::new();
        assert_eq!(expand_str(&mut env, "${x:=hello}"), ["hello"]);
        assert_eq!(env.get("x").map(|v| v.value().to_owned()), Some("hello".into()));
    }

    #[test]
    fn param_alt() {
        let mut env = EnvStack::new();
        assert_eq!(expand_str(&mut env, "${unset+alt}"), Vec::<String>::new());
        env.set("x", EnvVar::new("value")).unwrap();
        assert_eq!(expand_str(&mut env, "${x+alt}"), ["alt"]);
        env.set("n", EnvVar::new("")).unwrap();
        assert_eq!(expand_str(&mut env, "${n:+alt}"), Vec::<String>::new());
        assert_eq!(expand_str(&mut env, "${n+alt}"), ["alt"]);
    }

    #[test]
    fn param_case_conversion() {
        let mut env = EnvStack::new();
        env.set("x", EnvVar::new("hELLo")).unwrap();
        assert_eq!(expand_str(&mut env, "${x^^}"), ["HELLO"]);
        assert_eq!(expand_str(&mut env, "${x,,}"), ["hello"]);
        assert_eq!(expand_str(&mut env, "${x^}"), ["HELLo"]);
        assert_eq!(expand_str(&mut env, "${x,}"), ["hELLo"]);
    }

    #[test]
    fn param_prefix_suffix_removal() {
        let mut env = EnvStack::new();
        env.set("y", EnvVar::new("a/b/c")).unwrap();
        assert_eq!(expand_str(&mut env, "${y#*/}"), ["b/c"]);
        assert_eq!(expand_str(&mut env, "${y##*/}"), ["c"]);
        assert_eq!(expand_str(&mut env, "${y%/*}"), ["a/b"]);
        assert_eq!(expand_str(&mut env, "${y%%/*}"), ["a"]);
    }

    #[test]
    fn glob_metachars_in_parameter_result_do_not_expand() {
        let mut env = EnvStack::new();
        env.set("x", EnvVar::new("*")).unwrap();
        assert_eq!(expand_str(&mut env, "$x"), ["*"]);
    }
}
