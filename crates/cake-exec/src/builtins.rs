//! Builtin commands. M2a covers the core set; the rest arrive in M2d.

use alloc::borrow::ToOwned;
use alloc::string::{String, ToString};

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
        while let Some(n) = chars.next() {
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
        // For simplicity, M2a prints each spec once (no cycling).
        break;
    }
    // Any trailing literal text.
    buf.push_str(&chars.as_str());
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
                if let Some(h) = chars.next() {
                    if ('0'..='7').contains(&h) {
                        oct.push(h);
                    }
                }
            }
            let val = u32::from_str_radix(&oct, 8).unwrap_or(0);
            char::from_u32(val).map(|c| c.to_string()).unwrap_or_default()
        }
        Some('x') => {
            let mut hex = String::new();
            for _ in 0..2 {
                if let Some(h) = chars.next() {
                    if h.is_ascii_hexdigit() {
                        hex.push(h);
                    }
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
            None => match exec.env.get(arg) {
                Some(var) => {
                    let mut flags = var.flags();
                    flags.insert(EnvVarFlags::READONLY);
                    exec.env
                        .set(arg, var.set_flags(flags))
                        .map_err(|e| alloc::format!("cake: readonly: {e}"))?;
                }
                None => {}
            },
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
            | "export" | "unset" | "readonly" | "shift" | "command"
    )
}

/// All builtin names, for `--help`.
pub fn builtin_names() -> &'static [&'static str] {
    &[
        "echo", "printf", "true", ":", "false", "exit", "cd", "pwd", "type", "export",
        "unset", "readonly", "shift", "command",
    ]
}
