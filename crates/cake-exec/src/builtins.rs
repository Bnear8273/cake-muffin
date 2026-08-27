//! Builtin commands. M2a covers the core set; the rest arrive in M2d.

use alloc::borrow::ToOwned;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use cake_env::{EnvVar, EnvVarFlags};
use cake_proc::ProcStatus;

use crate::executor::Executor;

/// Run `name` with `args` (args[0] == name). Returns `None` if `name` is not
/// a builtin.
pub fn run_builtin(exec: &mut Executor, name: &str, args: &[String]) -> Option<Result<ProcStatus, String>> {
    Some(match name {
        "echo" => echo(args),
        "printf" => printf(args),
        "true" | ":" => Ok(ProcStatus::Exit(0)),
        "false" => Ok(ProcStatus::Exit(1)),
        "exit" => exit(exec, args),
        "cd" => cd(exec, args),
        "pwd" => pwd(),
        "type" => r#type(exec, args),
        "export" => export(exec, args),
        "unset" => unset(exec, args),
        "readonly" => readonly(exec, args),
        "shift" => shift(exec, args),
        "command" => Ok(ProcStatus::Exit(0)),
        "alias" => alias(exec, args),
        "unalias" => unalias(exec, args),
        "test" => test_builtin(args),
        "[" => test_builtin(args),
        "break" => r#break(exec, args),
        "continue" => continue_(exec, args),
        "return" => r#return(exec, args),
        "source" => source(exec, args),
        "." => source(exec, args),
        "read" => read(exec, args),
        _ => return None,
    })
}

fn out(s: &str) {
    let _ = cake_platform::get().write(1, s.as_bytes());
}

// --- echo ---

fn echo(args: &[String]) -> Result<ProcStatus, String> {
    let mut newline = true;
    let mut escape = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-n" => {
                newline = false;
                i += 1;
            }
            "-e" => {
                escape = true;
                i += 1;
            }
            "-E" => {
                escape = false;
                i += 1;
            }
            "--" => {
                i += 1;
                break;
            }
            other if other.starts_with('-') && other.len() > 1 => {
                // Consume combined option letters (e.g. -ne).
                let mut letters = other[1..].chars();
                let mut matched = false;
                for c in letters.by_ref() {
                    match c {
                        'n' => {
                            newline = false;
                            matched = true;
                        }
                        'e' => {
                            escape = true;
                            matched = true;
                        }
                        'E' => {
                            escape = false;
                            matched = true;
                        }
                        _ => {}
                    }
                }
                if matched {
                    i += 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    let mut buf = String::new();
    for (idx, a) in args[i..].iter().enumerate() {
        if idx > 0 {
            buf.push(' ');
        }
        if escape {
            buf.push_str(&echo_escapes(a));
        } else {
            buf.push_str(a);
        }
    }
    if newline {
        buf.push('\n');
    }
    out(&buf);
    Ok(ProcStatus::Exit(0))
}

fn echo_escapes(s: &str) -> String {
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
            Some('c') => return out,
            Some('e') | Some('E') => out.push('\x1b'),
            Some('f') => out.push('\x0c'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('v') => out.push('\x0b'),
            Some('\\') => out.push('\\'),
            Some('0') => {
                let mut oct = String::from("0");
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
    out
}

// --- printf ---

fn printf(args: &[String]) -> Result<ProcStatus, String> {
    if args.len() < 2 {
        return Err("cake: printf: usage: printf FORMAT [ARG...]".into());
    }
    let fmt = &args[1];
    let mut buf = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' && c != '\\' {
            buf.push(c);
            continue;
        }
        if c == '\\' {
            buf.push_str(&printf_escape(&mut chars));
            continue;
        }
        // Parse a printf format spec minimally.
        let mut spec = String::from("%");
        for n in chars.by_ref() {
            spec.push(n);
            if n.is_ascii_alphabetic() {
                break;
            }
        }
        let conv = spec.chars().last().unwrap_or('%');
        let _has_flags = spec.len() > 1;
        match conv {
            's' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("");
                buf.push_str(arg);
            }
            'd' | 'i' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("0");
                let n: i64 = arg.parse().unwrap_or(0);
                buf.push_str(&alloc::format!("{n}"));
            }
            'u' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("0");
                let n: u64 = arg.parse().unwrap_or(0);
                buf.push_str(&alloc::format!("{n}"));
            }
            'x' | 'X' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("0");
                let n: u64 = arg.parse().unwrap_or(0);
                if conv == 'X' {
                    buf.push_str(&alloc::format!("{n:X}"));
                } else {
                    buf.push_str(&alloc::format!("{n:x}"));
                }
            }
            'o' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("0");
                let n: u64 = arg.parse().unwrap_or(0);
                buf.push_str(&alloc::format!("{n:o}"));
            }
            'f' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("0");
                let n: f64 = arg.parse().unwrap_or(0.0);
                buf.push_str(&alloc::format!("{n}"));
            }
            'c' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("");
                buf.push_str(arg.chars().next().map(|c| c.to_string()).unwrap_or_default().as_str());
            }
            '%' => buf.push('%'),
            'b' => {
                let arg = args.get(2).map(String::as_str).unwrap_or("");
                buf.push_str(&echo_escapes(arg));
            }
            _ => buf.push_str(&spec),
        }
        // M2a: each spec consumes the first argument (no cycling yet).
    }
    // Any trailing literal text.
    buf.push_str(chars.as_str());
    out(&buf);
    Ok(ProcStatus::Exit(0))
}

fn printf_escape(chars: &mut core::str::Chars<'_>) -> String {
    match chars.next() {
        None => String::from("\\"),
        Some('a') => "\x07".into(),
        Some('b') => "\x08".into(),
        Some('e') | Some('E') => "\x1b".into(),
        Some('f') => "\x0c".into(),
        Some('n') => "\n".into(),
        Some('r') => "\r".into(),
        Some('t') => "\t".into(),
        Some('v') => "\x0b".into(),
        Some('\\') => "\\".into(),
        Some('0') => {
            let mut oct = String::from("0");
            for _ in 0..2 {
                if let Some(h) = chars.next()
                    && ('0'..='7').contains(&h)
                {
                    oct.push(h);
                }
            }
            let val = u32::from_str_radix(&oct, 8).unwrap_or(0);
            char::from_u32(val).map(|c| c.to_string()).unwrap_or_default()
        }
        Some('x') => {
            let mut hex = String::new();
            for _ in 0..2 {
                if let Some(h) = chars.next()
                    && h.is_ascii_hexdigit()
                {
                    hex.push(h);
                }
            }
            let val = u32::from_str_radix(&hex, 16).unwrap_or(0);
            char::from_u32(val).map(|c| c.to_string()).unwrap_or_default()
        }
        Some(other) => other.to_string(),
    }
}

// --- exit ---

fn exit(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    let code = match args.get(1) {
        Some(s) => match s.parse::<i32>() {
            Ok(n) => n & 0xff,
            Err(_) => return Err(alloc::format!("cake: exit: {s}: numeric argument required")),
        },
        None => exec.last_status.status_code(),
    };
    exec.request_exit(code);
    Ok(ProcStatus::Exit(code))
}

// --- cd ---

fn cd(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    let old = cake_platform::get().current_dir();
    let target = match args.get(1) {
        None => match exec.env.get("HOME") {
            Some(v) => v.value().to_owned(),
            None => return Err("cake: cd: HOME not set".into()),
        },
        Some(d) if d == "-" => match exec.env.get("OLDPWD") {
            Some(v) => {
                out(&alloc::format!("{}\n", v.value()));
                v.value().to_owned()
            }
            None => return Err("cake: cd: OLDPWD not set".into()),
        },
        Some(d) => d.clone(),
    };
    cake_platform::get()
        .set_current_dir(&target)
        .map_err(|e| alloc::format!("cake: cd: {target}: {e}"))?;
    exec.env
        .set("OLDPWD", EnvVar::new(old).set_flags(EnvVarFlags::EXPORT))
        .ok();
    exec.env
        .set("PWD", EnvVar::new(cake_platform::get().current_dir()).set_flags(EnvVarFlags::EXPORT))
        .ok();
    Ok(ProcStatus::Exit(0))
}

// --- pwd ---

fn pwd() -> Result<ProcStatus, String> {
    out(&alloc::format!("{}\n", cake_platform::get().current_dir()));
    Ok(ProcStatus::Exit(0))
}

// --- alias / unalias ---
//
// Aliases are session-scoped (like bash, which re-defines them from rc files
// each session). cake has no rc file yet, so define them in the interactive
// REPL, e.g.:
//
//   alias ls='ls --color=auto'
//   alias grep='grep --color=auto'
//   alias diff='diff --color=auto'
//
// # Why colour aliases?
//
// cake's child processes inherit the terminal stdout, so tools that detect a
// tty themselves (git, rg, bat, fzf) colourise automatically. But GNU
// coreutils tools require an explicit flag that shells usually provide via
// aliases: `ls --color=auto`, `grep --color=auto`, `diff --color=auto`,
// `gcc -fdiagnostics-color=auto`, `ip -color=auto`, ...
//
// Use `--color=auto`/`--color=tty` (colourise only on a tty) rather than
// `--color=always`, so piped output (`ls | grep`) stays clean.
//
// Platform note: BSD/macOS `ls` colours via the `CLICOLOR` env var + `-G`
// flag instead of `--color`; if a macOS backend ever appears, the aliases
// here would use `ls -G` and the other `-G`-style flags.

/// `alias [name[=value] ...]`, `alias -p`, or bare `alias`.
fn alias(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    let mut i = 1;
    if args.get(1).is_some_and(|a| a == "-p") {
        i = 2;
    }
    if i >= args.len() {
        for (name, value) in &exec.aliases {
            print_alias(name, value);
        }
        return Ok(ProcStatus::Exit(0));
    }
    for arg in &args[i..] {
        match arg.find('=') {
            Some(eq) => {
                let name = &arg[..eq];
                let value = &arg[eq + 1..];
                exec.aliases.insert(name.to_owned(), value.to_owned());
            }
            None => match exec.aliases.get(arg) {
                Some(value) => print_alias(arg, value),
                None => return Err(alloc::format!("cake: alias: {arg}: not found")),
            },
        }
    }
    Ok(ProcStatus::Exit(0))
}

/// `unalias name...` or `unalias -a`.
fn unalias(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    if args.get(1).is_some_and(|a| a == "-a") {
        exec.aliases.clear();
        return Ok(ProcStatus::Exit(0));
    }
    for name in &args[1..] {
        if exec.aliases.remove(name).is_none() {
            return Err(alloc::format!("cake: unalias: {name}: not found"));
        }
    }
    Ok(ProcStatus::Exit(0))
}

fn print_alias(name: &str, value: &str) {
    out(&alloc::format!("alias {name}='{}'\n", quote_single(value)));
}

/// Quote `s` for a single-quoted shell word (escaping `'` as `'\''`).
fn quote_single(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out
}

// --- test / [ ---

/// `test EXPR` and `[ EXPR ]`. The arguments arrive already word-split and
/// expanded by the shell.
fn test_builtin(args: &[String]) -> Result<ProcStatus, String> {
    let is_bracket = args.first().map(String::as_str) == Some("[");
    let mut expr = &args[1..];
    if is_bracket {
        if expr.last().map(String::as_str) != Some("]") {
            return Err("cake: [: missing `]'".into());
        }
        expr = &expr[..expr.len() - 1];
    }
    if expr.is_empty() {
        // Bare `test` / `[ ]` with no expression is false.
        return Ok(ProcStatus::Exit(1));
    }
    let val = eval_test_expr(expr)?;
    Ok(ProcStatus::Exit(if val { 0 } else { 1 }))
}

/// Evaluate a `test`/`[ ]` expression over already-expanded arguments.
///
/// Grammar: `expr := and (-o and)* ; and := not (-a not)* ; not := '!' not |
/// primary ; primary := unary-op arg | arg binop arg | arg`.
fn eval_test_expr(args: &[String]) -> Result<bool, String> {
    let mut p = TestParser { args, pos: 0 };
    let val = p.parse_or()?;
    if p.pos != args.len() {
        return Err(alloc::format!(
            "cake: test: unexpected argument `{}'",
            args[p.pos]
        ));
    }
    Ok(val)
}

struct TestParser<'a> {
    args: &'a [String],
    pos: usize,
}

impl TestParser<'_> {
    fn peek(&self) -> Option<&str> {
        self.args.get(self.pos).map(String::as_str)
    }

    fn eat(&mut self) -> Option<String> {
        let t = self.args.get(self.pos).cloned()?;
        self.pos += 1;
        Some(t)
    }

    fn parse_or(&mut self) -> Result<bool, String> {
        let mut val = self.parse_and()?;
        while self.peek() == Some("-o") {
            self.eat();
            let rhs = self.parse_and()?;
            val = val || rhs;
        }
        Ok(val)
    }

    fn parse_and(&mut self) -> Result<bool, String> {
        let mut val = self.parse_not()?;
        while self.peek() == Some("-a") {
            self.eat();
            let rhs = self.parse_not()?;
            val = val && rhs;
        }
        Ok(val)
    }

    fn parse_not(&mut self) -> Result<bool, String> {
        if self.peek() == Some("!") {
            self.eat();
            return Ok(!self.parse_not()?);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<bool, String> {
        if let Some(op) = self.peek().map(String::from)
            && is_unary_op(&op)
        {
            self.eat();
            let arg = self
                .eat()
                .ok_or_else(|| "cake: test: argument expected".to_string())?;
            return unary_test(&op, &arg);
        }
        let lhs = self
            .eat()
            .ok_or_else(|| "cake: test: expression expected".to_string())?;
        match self.peek().map(String::from) {
            Some(bin) if is_binary_op(&bin) => {
                self.eat();
                let rhs = self
                    .eat()
                    .ok_or_else(|| "cake: test: argument expected".to_string())?;
                binary_test(&lhs, &bin, &rhs)
            }
            _ => Ok(!lhs.is_empty()),
        }
    }
}

fn is_unary_op(op: &str) -> bool {
    matches!(
        op,
        "-n" | "-z" | "-e" | "-f" | "-d" | "-r" | "-w" | "-x" | "-s" | "-L"
    )
}

fn is_binary_op(op: &str) -> bool {
    matches!(
        op,
        "=" | "==" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" | "<" | ">"
    )
}

fn unary_test(op: &str, arg: &str) -> Result<bool, String> {
    Ok(match op {
        "-n" => !arg.is_empty(),
        "-z" => arg.is_empty(),
        _ => {
            let p = cake_platform::get();
            match op {
                "-e" => p.stat(arg).exists,
                "-f" => p.stat(arg).is_file,
                "-d" => p.stat(arg).is_dir,
                "-r" => p.stat(arg).is_readable,
                "-w" => p.stat(arg).is_writable,
                "-x" => p.stat(arg).is_executable,
                "-s" => p.stat(arg).size > 0,
                "-L" => p.stat(arg).is_symlink,
                _ => false,
            }
        }
    })
}

fn binary_test(lhs: &str, op: &str, rhs: &str) -> Result<bool, String> {
    Ok(match op {
        "=" | "==" => lhs == rhs,
        "!=" => lhs != rhs,
        "-eq" => parse_num(lhs) == parse_num(rhs),
        "-ne" => parse_num(lhs) != parse_num(rhs),
        "-lt" => parse_num(lhs) < parse_num(rhs),
        "-le" => parse_num(lhs) <= parse_num(rhs),
        "-gt" => parse_num(lhs) > parse_num(rhs),
        "-ge" => parse_num(lhs) >= parse_num(rhs),
        "<" => lhs < rhs,
        ">" => lhs > rhs,
        _ => false,
    })
}

fn parse_num(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
}

// --- break / continue / return ---

/// `break [n]` — exit the enclosing `for`/`while`/`until` loop.
fn r#break(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    if exec.loop_depth == 0 {
        return Err("cake: break: only meaningful in a `for`, `while`, or `until` loop".into());
    }
    let depth = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    exec.loop_control = Some(crate::executor::LoopControl { is_break: true, depth });
    Ok(ProcStatus::Exit(0))
}

/// `continue [n]` — skip to the next iteration of the enclosing loop.
fn continue_(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    if exec.loop_depth == 0 {
        return Err(
            "cake: continue: only meaningful in a `for`, `while`, or `until` loop".into(),
        );
    }
    let depth = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    exec.loop_control = Some(crate::executor::LoopControl { is_break: false, depth });
    Ok(ProcStatus::Exit(0))
}

/// `return [n]` — exit the enclosing function/sourced file with status `n`.
fn r#return(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    if exec.fn_depth == 0 {
        return Err("cake: return: can only `return' from a function or sourced script".into());
    }
    let code = match args.get(1) {
        Some(s) => match s.parse::<i32>() {
            Ok(n) => n,
            Err(_) => return Err(alloc::format!("cake: return: `{s}': numeric argument required")),
        },
        None => exec.last_status.status_code(),
    };
    exec.return_requested = Some(code & 0xff);
    Ok(ProcStatus::Exit(code & 0xff))
}

// --- source / . ---

/// `source file [args...]` — read and execute `file` in the current shell.
fn source(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    let Some(path) = args.get(1) else {
        return Err("cake: source: filename argument required".into());
    };
    let content = read_file(path)
        .map_err(|e| alloc::format!("cake: source: {path}: {e}"))?;
    // The sourced file sees args as positional (`$1`, ...); `return` works.
    let saved_positional = core::mem::replace(
        &mut exec.positional,
        alloc::vec![path.clone()].into_iter().chain(args[2..].iter().cloned()).collect(),
    );
    exec.fn_depth += 1;
    let outcome = exec.eval_str(&content);
    exec.fn_depth -= 1;
    exec.positional = saved_positional;
    if let Some(err) = outcome.error {
        return Err(err);
    }
    Ok(outcome.status)
}

/// Read an entire file into a string via the platform fd layer.
fn read_file(path: &str) -> Result<String, String> {
    let p = cake_platform::get();
    let fd = p
        .open_file(path, cake_platform::FileOpenMode::Read)
        .map_err(|e| alloc::format!("{e}"))?;
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match p.read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let _ = p.close(fd);
    Ok(String::from_utf8_lossy(&out).into_owned())
}

// --- read ---

/// `read [-r] var...` — read a line from stdin, split on IFS, assign to vars.
/// The last var receives any remaining fields.
fn read(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    let mut i = 1;
    let mut raw = false;
    if args.get(1).is_some_and(|a| a == "-r") {
        raw = true;
        i = 2;
    }
    let vars = &args[i..];
    if vars.is_empty() {
        return Err("cake: read: usage: read [-r] var...".into());
    }
    let line = read_line(raw)?;
    let ifs: Vec<char> = match exec.env.get("IFS") {
        Some(v) => v.value().chars().collect(),
        None => {
            vec![' ', '\t', '\n']
        }
    };
    let fields: Vec<String> = line
        .split(|c: char| ifs.contains(&c))
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for (idx, var) in vars.iter().enumerate() {
        let value = if idx + 1 == vars.len() {
            fields[idx..].join(" ")
        } else {
            fields.get(idx).cloned().unwrap_or_default()
        };
        let _ = exec.env.set(var, cake_env::EnvVar::new(value));
    }
    Ok(ProcStatus::Exit(0))
}

/// Read one line (up to `\n`, excluded) from fd 0.
fn read_line(raw: bool) -> Result<String, String> {
    let p = cake_platform::get();
    let mut out = String::new();
    let mut buf = [0u8; 1];
    let mut escaped = false;
    loop {
        match p.read(0, &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let b = buf[0];
                if escaped {
                    out.push(b as char);
                    escaped = false;
                } else if b == b'\\' && !raw {
                    escaped = true;
                } else if b == b'\n' {
                    break;
                } else {
                    out.push(b as char);
                }
            }
            Err(_) => break,
        }
    }
    Ok(out)
}

// --- type ---

fn r#type(exec: &Executor, args: &[String]) -> Result<ProcStatus, String> {
    if args.len() < 2 {
        return Err("cake: type: usage: type NAME...".into());
    }
    let mut all_ok = true;
    for name in &args[1..] {
        let kind = crate::resolve::describe(exec, name);
        out(&alloc::format!("{name} is {kind}\n"));
        if kind.starts_with("cake:") {
            all_ok = false;
        }
    }
    Ok(ProcStatus::Exit(if all_ok { 0 } else { 1 }))
}

// --- export ---

fn export(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    if args.len() == 1 {
        // `export` with no args prints all exported vars.
        for (name, var) in exec.env.get_names_exported() {
            out(&alloc::format!("export {name}=\"{}\"\n", var.value()));
        }
        return Ok(ProcStatus::Exit(0));
    }
    for arg in &args[1..] {
        if arg.starts_with('-') {
            continue; // options: not handled in M2a
        }
        match arg.find('=') {
            Some(eq) => {
                let name = &arg[..eq];
                let val = &arg[eq + 1..];
                if !is_identifier(name) {
                    return Err(alloc::format!("cake: export: `{arg}': not a valid identifier"));
                }
                exec.env
                    .set(name, EnvVar::new(val).set_flags(EnvVarFlags::EXPORT))
                    .map_err(|e| alloc::format!("cake: export: {e}"))?;
            }
            None => {
                // Mark an existing variable as exported.
                match exec.env.get(arg) {
                    Some(var) => {
                        let mut flags = var.flags();
                        flags.insert(EnvVarFlags::EXPORT);
                        exec.env
                            .set(arg, var.set_flags(flags))
                            .map_err(|e| alloc::format!("cake: export: {e}"))?;
                    }
                    None => {
                        if !is_identifier(arg) {
                            return Err(alloc::format!("cake: export: `{arg}': not a valid identifier"));
                        }
                        exec.env
                            .set(arg, EnvVar::new("").set_flags(EnvVarFlags::EXPORT))
                            .map_err(|e| alloc::format!("cake: export: {e}"))?;
                    }
                }
            }
        }
    }
    Ok(ProcStatus::Exit(0))
}

// --- unset ---

fn unset(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    for arg in &args[1..] {
        if arg.starts_with('-') {
            continue;
        }
        exec.env.remove(arg);
    }
    Ok(ProcStatus::Exit(0))
}

// --- readonly ---

fn readonly(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    for arg in &args[1..] {
        if arg.starts_with('-') {
            continue;
        }
        match arg.find('=') {
            Some(eq) => {
                let name = &arg[..eq];
                let val = &arg[eq + 1..];
                exec.env
                    .set(name, EnvVar::new(val).set_flags(EnvVarFlags::READONLY))
                    .map_err(|e| alloc::format!("cake: readonly: {e}"))?;
            }
            None => {
                if let Some(var) = exec.env.get(arg) {
                    let mut flags = var.flags();
                    flags.insert(EnvVarFlags::READONLY);
                    exec.env
                        .set(arg, var.set_flags(flags))
                        .map_err(|e| alloc::format!("cake: readonly: {e}"))?;
                }
            }
        }
    }
    Ok(ProcStatus::Exit(0))
}

// --- shift ---

fn shift(exec: &mut Executor, args: &[String]) -> Result<ProcStatus, String> {
    let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    if n > exec.positional.len().saturating_sub(1) {
        return Err("cake: shift: shift count out of range".into());
    }
    exec.positional.drain(1..1 + n);
    Ok(ProcStatus::Exit(0))
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "echo" | "printf" | "true" | ":" | "false" | "exit" | "cd" | "pwd" | "type"
            | "export" | "unset" | "readonly" | "shift" | "command" | "alias" | "unalias"
            | "test" | "[" | "break" | "continue" | "return" | "source" | "." | "read"
    )
}

/// All builtin names, for `--help`.
pub fn builtin_names() -> &'static [&'static str] {
    &[
        "echo", "printf", "true", ":", "false", "exit", "cd", "pwd", "type", "export",
        "unset", "readonly", "shift", "command", "alias", "unalias", "test", "[", "break",
        "continue", "return", "source", ".", "read",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;
    use alloc::string::ToString;
    use alloc::vec::Vec;
    use cake_env::EnvStack;

    #[test]
    fn alias_defines() {
        let mut exec = Executor::new(EnvStack::new());
        let r = alias(&mut exec, &["alias".into(), "ls=ls --color=auto".into()]).unwrap();
        assert_eq!(r, ProcStatus::Exit(0));
        assert_eq!(exec.aliases.get("ls").map(String::as_str), Some("ls --color=auto"));
    }

    #[test]
    fn alias_query_missing_errors() {
        let mut exec = Executor::new(EnvStack::new());
        let r = alias(&mut exec, &["alias".into(), "nope".into()]);
        assert!(r.is_err());
    }

    #[test]
    fn unalias_removes() {
        let mut exec = Executor::new(EnvStack::new());
        exec.aliases.insert("ls".into(), "ls --color=auto".into());
        let r = unalias(&mut exec, &["unalias".into(), "ls".into()]).unwrap();
        assert_eq!(r, ProcStatus::Exit(0));
        assert!(exec.aliases.is_empty());
    }

    #[test]
    fn unalias_missing_errors() {
        let mut exec = Executor::new(EnvStack::new());
        let r = unalias(&mut exec, &["unalias".into(), "nope".into()]);
        assert!(r.is_err());
    }

    #[test]
    fn unalias_dash_a_clears_all() {
        let mut exec = Executor::new(EnvStack::new());
        exec.aliases.insert("ls".into(), "ls --color=auto".into());
        exec.aliases.insert("grep".into(), "grep --color=auto".into());
        let r = unalias(&mut exec, &["unalias".into(), "-a".into()]).unwrap();
        assert_eq!(r, ProcStatus::Exit(0));
        assert!(exec.aliases.is_empty());
    }

    #[test]
    fn quote_single_escapes_embedded_quotes() {
        assert_eq!(quote_single("it's"), "it'\\''s");
        assert_eq!(quote_single("plain"), "plain");
    }

    #[test]
    fn test_binary_and_unary() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // 1 = 2 → false (exit 1)
        assert_eq!(test_builtin(&a(&["test", "1", "=", "2"])).unwrap(), ProcStatus::Exit(1));
        // 1 = 1 → true
        assert_eq!(test_builtin(&a(&["test", "1", "=", "1"])).unwrap(), ProcStatus::Exit(0));
        // 1 != 2 → true
        assert_eq!(test_builtin(&a(&["test", "1", "!=", "2"])).unwrap(), ProcStatus::Exit(0));
        // 2 -gt 1 → true
        assert_eq!(test_builtin(&a(&["test", "2", "-gt", "1"])).unwrap(), ProcStatus::Exit(0));
        // -n x → true
        assert_eq!(test_builtin(&a(&["test", "-n", "x"])).unwrap(), ProcStatus::Exit(0));
        // -n "" → false
        assert_eq!(test_builtin(&a(&["test", "-n", ""])).unwrap(), ProcStatus::Exit(1));
        // ! expression
        assert_eq!(test_builtin(&a(&["test", "!", "1", "=", "2"])).unwrap(), ProcStatus::Exit(0));
        // [ 1 = 1 ] → true
        assert_eq!(test_builtin(&a(&["[", "1", "=", "1", "]"])).unwrap(), ProcStatus::Exit(0));
        // bare `test` with no args → false
        assert_eq!(test_builtin(&a(&["test"])).unwrap(), ProcStatus::Exit(1));
    }
}
