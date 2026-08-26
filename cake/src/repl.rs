//! The interactive REPL.
//!
//! `cake` with no arguments enters this loop: a PS1 prompt, line editing and
//! history via rustyline, syntax highlighting, tab completion, continuation
//! (PS2) for incomplete input, and Ctrl-C / Ctrl-D handling. The [`Executor`]
//! is kept across commands (via `Rc<RefCell<..>>`) so variables, functions
//! and the working directory persist.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;
use std::rc::Rc;

use cake_blacklist::CommandBlacklist;
use cake_complete::CompleteKind;
use cake_exec::Executor;
use cake_platform::XdgKind;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, EditMode, Editor, Helper};

use crate::import_env;

/// rustyline helper: syntax highlighting and tab completion. Autosuggestion
/// (Hinter) arrives in M3c.
struct CakeHelper {
    exec: Rc<RefCell<Executor>>,
}

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
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let exec = self.exec.borrow();
        Ok(complete_in(&exec, line, pos))
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

/// Build completion candidates for the word under the cursor.
fn complete_in(exec: &Executor, line: &str, pos: usize) -> (usize, Vec<Pair>) {
    let (word_start, word) = cake_complete::current_word(line, pos);
    let kind = cake_complete::classify(line, pos);

    match kind {
        CompleteKind::Command => {
            let mut cands: Vec<String> = Vec::new();
            for b in cake_exec::builtins::builtin_names() {
                cands.push((*b).to_string());
            }
            for f in exec.functions.keys() {
                cands.push(f.clone());
            }
            if let Some(pathvar) = exec.env.get("PATH") {
                for dir in pathvar.values() {
                    if let Ok(rd) = std::fs::read_dir(dir) {
                        for e in rd.flatten() {
                            let name = e.file_name().to_string_lossy().into_owned();
                            if cake_platform::get().is_executable(&e.path().to_string_lossy()) {
                                cands.push(name);
                            }
                        }
                    }
                }
            }
            if let Ok(rd) = std::fs::read_dir(".") {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if cake_platform::get().is_executable(&e.path().to_string_lossy()) {
                        cands.push(name);
                    }
                }
            }
            let matches = dedup(cake_complete::filter_candidates(word, &cands));
            let pairs = matches
                .into_iter()
                .map(|c| Pair {
                    display: c.clone(),
                    replacement: format!("{c} "),
                })
                .collect();
            (word_start, pairs)
        }
        CompleteKind::File => {
            // Split the word into (dir, base) around the last `/`.
            let (dir, base, dir_prefix) = match word.rfind('/') {
                Some(0) => ("/", &word[1..], "/"),
                Some(i) => (&word[..i], &word[i + 1..], &word[..=i]),
                None => (".", word, ""),
            };
            let mut cands: Vec<String> = Vec::new();
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let full = e.path().to_string_lossy().into_owned();
                    if cake_platform::get().stat(&full).is_dir {
                        cands.push(format!("{name}/"));
                    } else {
                        cands.push(name);
                    }
                }
            }
            let matches = dedup(cake_complete::filter_candidates(base, &cands));
            let pairs = matches
                .into_iter()
                .map(|c| Pair {
                    display: c.clone(),
                    replacement: format!("{dir_prefix}{c}"),
                })
                .collect();
            (word_start, pairs)
        }
        CompleteKind::Variable => {
            let prefix = word.strip_prefix('$').unwrap_or(word);
            let names: Vec<String> = exec.env.get_names().iter().map(|n| n.to_string()).collect();
            let matches = dedup(cake_complete::filter_candidates(prefix, &names));
            let pairs = matches
                .into_iter()
                .map(|c| Pair {
                    display: format!("${c}"),
                    replacement: c,
                })
                .collect();
            // Keep the leading `$` in the line; replace only the name part.
            (word_start + 1, pairs)
        }
    }
}

fn dedup(v: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for s in v {
        if seen.insert(s.clone()) {
            out.push(s);
        }
    }
    out
}

/// Run the interactive loop. Never returns.
pub fn run_interactive() -> ! {
    let executor = Rc::new(RefCell::new(Executor::new(import_env())));
    {
        let mut exec = executor.borrow_mut();
        exec.shell_pid = std::process::id() as i32;
        load_blacklist(&mut exec);
    }

    // When stdin is not a terminal (e.g. `echo 'ls' | cake`), read plain
    // lines instead of using the raw-mode line editor.
    if !std::io::stdin().is_terminal() {
        let mut exec = executor.borrow_mut();
        run_piped(&mut exec);
    }

    let helper = CakeHelper {
        exec: executor.clone(),
    };
    let mut editor = match Editor::<CakeHelper, rustyline::history::DefaultHistory>::with_config(
        Config::builder()
            .edit_mode(EditMode::Emacs)
            .completion_type(CompletionType::List)
            .build(),
    ) {
        Ok(mut e) => {
            e.set_helper(Some(helper));
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
        let mut prompt = {
            let exec = executor.borrow();
            ps(&exec, "PS1", "$ ")
        };
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
                        let exec = executor.borrow();
                        save_blacklist(&exec);
                        std::process::exit(0);
                    }
                    // Ctrl-D during a continuation discards the input.
                    buffer.clear();
                    prompt = {
                        let exec = executor.borrow();
                        ps(&exec, "PS1", "$ ")
                    };
                    continue;
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl-C: cancel the current line(s), start over.
                    println!();
                    buffer.clear();
                    prompt = {
                        let exec = executor.borrow();
                        ps(&exec, "PS1", "$ ")
                    };
                    continue;
                }
                Err(e) => {
                    eprintln!("cake: {e}");
                    std::process::exit(1);
                }
            }

            match cake_syntax::parse(&buffer) {
                Ok(_) => {
                    let outcome = {
                        let mut exec = executor.borrow_mut();
                        exec.eval_str(&buffer)
                    };
                    if let Some(err) = outcome.error {
                        eprintln!("{err}");
                    }
                    let code = {
                        let mut exec = executor.borrow_mut();
                        exec.take_exit_requested()
                    };
                    if let Some(code) = code {
                        save_history(&mut editor);
                        let exec = executor.borrow();
                        save_blacklist(&exec);
                        std::process::exit(code);
                    }
                    break;
                }
                Err(errs) => {
                    if errs.iter().any(|e| e.is_incomplete) {
                        // Need more input: continue with the PS2 prompt.
                        let exec = executor.borrow();
                        prompt = ps(&exec, "PS2", "> ");
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
