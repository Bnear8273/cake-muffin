//! The interactive REPL.
//!
//! `cake` with no arguments enters this loop: a PS1 prompt, line editing and
//! history via rustyline, continuation (PS2) for incomplete input, and
//! Ctrl-C / Ctrl-D handling. The [`Executor`] is kept across commands so
//! variables, functions and the working directory persist.

use std::borrow::Cow;
use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;

use cake_blacklist::CommandBlacklist;
use cake_exec::Executor;
use cake_platform::XdgKind;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, EditMode, Editor, Helper};

use crate::import_env;

/// rustyline helper: syntax highlighting now; completion/hints/validation
/// grow in M3b/M3c.
#[derive(Clone, Default)]
struct CakeHelper;

impl Highlighter for CakeHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Owned(cake_highlight::highlight_line(line))
    }
    fn highlight_char(&self, _line: &str, _pos: usize, _kind: rustyline::highlight::CmdKind) -> bool {
        true
    }
}

impl Completer for CakeHelper {
    type Candidate = Pair;
    fn complete(
        &self,
        _line: &str,
        _pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        Ok((0, Vec::new()))
    }
}

impl Hinter for CakeHelper {
    type Hint = String;
    fn hint(&self, _line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        None
    }
}

impl Validator for CakeHelper {
    fn validate(&self, _ctx: &mut rustyline::validate::ValidationContext) -> rustyline::Result<rustyline::validate::ValidationResult> {
        Ok(rustyline::validate::ValidationResult::Valid(None))
    }
}

impl Helper for CakeHelper {}

/// Run the interactive loop. Never returns.
pub fn run_interactive() -> ! {
    let mut executor = Executor::new(import_env());
    executor.shell_pid = std::process::id() as i32;
    load_blacklist(&mut executor);

    // When stdin is not a terminal (e.g. `echo 'ls' | cake`), read plain
    // lines instead of using the raw-mode line editor.
    if !std::io::stdin().is_terminal() {
        run_piped(&mut executor);
    }

    let mut editor = match Editor::<CakeHelper, rustyline::history::DefaultHistory>::with_config(
        Config::builder()
            .edit_mode(EditMode::Emacs)
            .completion_type(CompletionType::List)
            .build(),
    ) {
        Ok(mut e) => {
            e.set_helper(Some(CakeHelper));
            e
        }
        Err(e) => {
            eprintln!("cake: failed to init line editor: {e}");
            std::process::exit(1);
        }
    };
    if let Some(path) = history_path() {
        let _ = editor.load_history(&path);
    }

    let mut buffer = String::new();
    loop {
        buffer.clear();
        let mut prompt = ps(&executor, "PS1", "$ ");
        loop {
            match editor.readline(&prompt) {
                Ok(line) => {
                    editor.add_history_entry(line.as_str()).ok();
                    buffer.push_str(&line);
                    buffer.push('\n');
                }
                Err(ReadlineError::Eof) => {
                    if buffer.is_empty() {
                        println!();
                        save_blacklist(&executor);
                        std::process::exit(0);
                    }
                    // Ctrl-D during a continuation discards the input.
                    buffer.clear();
                    prompt = ps(&executor, "PS1", "$ ");
                    continue;
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl-C: cancel the current line(s), start over.
                    println!();
                    buffer.clear();
                    prompt = ps(&executor, "PS1", "$ ");
                    continue;
                }
                Err(e) => {
                    eprintln!("cake: {e}");
                    std::process::exit(1);
                }
            }

            match cake_syntax::parse(&buffer) {
                Ok(_) => {
                    let outcome = executor.eval_str(&buffer);
                    if let Some(err) = outcome.error {
                        eprintln!("{err}");
                    }
                    if let Some(code) = executor.take_exit_requested() {
                        save_history(&mut editor);
                        save_blacklist(&executor);
                        std::process::exit(code);
                    }
                    break;
                }
                Err(errs) => {
                    if errs.iter().any(|e| e.is_incomplete) {
                        // Need more input: continue with the PS2 prompt.
                        prompt = ps(&executor, "PS2", "> ");
                        continue;
                    }
                    eprintln!("cake: {}", errs[0].message);
                    break;
                }
            }
        }
    }
}

/// Run one line at a time from a piped stdin (non-interactive fallback).
fn run_piped(executor: &mut Executor) -> ! {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let outcome = executor.eval_str(&line);
        if let Some(err) = outcome.error {
            eprintln!("{err}");
        }
        if let Some(code) = executor.take_exit_requested() {
            save_blacklist(executor);
            std::process::exit(code);
        }
    }
    save_blacklist(executor);
    std::process::exit(executor.last_status.status_code());
}

fn ps(exec: &Executor, name: &str, default: &str) -> String {
    exec.env
        .get(name)
        .map(|v| v.value().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn data_dir() -> PathBuf {
    PathBuf::from(cake_platform::get().xdg_dir(XdgKind::Data)).join("cake")
}

fn history_path() -> Option<PathBuf> {
    Some(data_dir().join("history"))
}

fn save_history(editor: &mut Editor<CakeHelper, rustyline::history::DefaultHistory>) {
    if let Some(path) = history_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = editor.save_history(&path);
    }
}

pub(crate) fn load_blacklist(exec: &mut Executor) {
    let path = data_dir().join("blacklist");
    if let Ok(text) = std::fs::read_to_string(path) {
        exec.blacklist = CommandBlacklist::from_text(&text);
    }
}

pub(crate) fn save_blacklist(exec: &Executor) {
    let path = data_dir().join("blacklist");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, exec.blacklist.to_text());
}
