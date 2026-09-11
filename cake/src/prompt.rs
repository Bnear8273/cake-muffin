//! p10k-style segmented prompt driver.
//!
//! Collects shell state (exit code, current directory, git status, elapsed
//! time, user name, local clock) from the [`Executor`] and [`Platform`], then
//! calls [`cake_prompt::render`] to produce the two-line ANSI prompt. The repl
//! driver decides whether to use this or the legacy PS1 prompt.

use cake_exec::Executor;
use cake_prompt::{Config, Facts, GitStatus};

/// Whether the segmented prompt is enabled. On by default; an explicitly
/// empty `CAKE_PROMPT_LEFT` disables it (falling back to the legacy
/// PS1/path prompt).
pub fn prompt_enabled(exec: &Executor) -> bool {
    match exec.env.get("CAKE_PROMPT_LEFT") {
        Some(v) => !v.value().trim().is_empty(),
        None => true,
    }
}

/// Build the prompt for the current shell state.
///
/// Returns `(prompt, right_prompt)`: the editor prompt is two lines
/// (`info line \n input prefix`), and the right-hand content is passed
/// separately so the editor can align it with a gap fill.
///
/// `elapsed_ms` is the wall time of the last command (None on the first prompt).
pub fn build_prompt(exec: &Executor, elapsed_ms: Option<u64>) -> (String, String) {
    let cfg = Config::from_env(&|name| exec.env.get(name).map(|v| v.value().to_owned()));
    if cfg.left_segments.is_empty() && cfg.right_segments.is_empty() {
        return (String::new(), String::new());
    }

    let user_str = exec.env.get("USER").map(|v| v.value().to_owned());
    let user = user_str.as_deref();
    let home_str = exec
        .env
        .get("HOME")
        .map(|v| v.value())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let dir_str = shorten_dir(&cake_platform::get().current_dir(), home_str.as_deref());
    let git = capture_git_status();
    let exit = exec.last_status.status_code();
    let signal = match &exec.last_status {
        cake_exec::ProcStatus::Signal(n) => Some(cake_platform::get().signal_name(*n)),
        _ => None,
    };
    let clock = {
        let (h, m, s) = cake_platform::get().local_time_hms();
        Some(format!("{h:02}:{m:02}:{s:02}"))
    };
    let facts = Facts {
        user,
        dir: &dir_str,
        git,
        exit,
        signal,
        elapsed_ms,
        clock: clock.as_deref(),
    };
    let r = cake_prompt::render(&facts, &cfg);
    (format!("{}\n{}", r.left, r.input), r.right)
}

/// Shorten a path by replacing `$HOME` with `~`.
fn shorten_dir(dir: &str, home: Option<&str>) -> String {
    match home {
        Some(h) if !h.is_empty() => {
            if let Some(rest) = dir.strip_prefix(h) {
                return if rest.is_empty() {
                    "~".to_owned()
                } else {
                    format!("~{rest}")
                };
            }
            dir.to_owned()
        }
        _ => dir.to_owned(),
    }
}

/// Run `git status --porcelain=v2 -b` and parse the output, or `None` if
/// we are not inside a git repository (or git is not installed).
fn capture_git_status() -> Option<GitStatus> {
    // `-c status.optionalLocks=false` avoids git refreshing its index for the
    // read-only status query (safe under a running editor).
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "status.optionalLocks=false",
            "status",
            "--porcelain=v2",
            "-b",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    cake_prompt::parse_status(text)
}
