//! The interactive REPL.
//!
//! `cake` with no arguments enters this loop: a PS1 prompt, line editing and
//! history via the in-house line editor ([`crate::readline`]), syntax
//! highlighting, tab completion, continuation (PS2) for incomplete input, and
//! Ctrl-C / Ctrl-D handling. The [`Executor`] is kept across commands (via
//! `Rc<RefCell<..>>`) so variables, functions and the working directory
//! persist.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;
use std::rc::Rc;

use cake_blacklist::CommandBlacklist;
use cake_complete::CompleteKind;
use cake_editor::Candidate;
use cake_editor::Editor as LineEditor;
use cake_editor::{Key, KeyParser};
use cake_exec::Executor;
use cake_platform::XdgKind;

use crate::import_env;
use crate::prompt::{build_prompt, prompt_enabled};
use crate::readline::{ReadOutcome, read_loop};

/// Build completion candidates for the word under the cursor.
fn complete_in(exec: &Executor, line: &str, pos: usize) -> (usize, Vec<Candidate>) {
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
            for a in exec.aliases.keys() {
                cands.push(a.clone());
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
            let cands: Vec<String> = cands
                .into_iter()
                .filter(|c| !exec.blacklist.contains(c))
                .collect();
            let matches = dedup(cake_complete::filter_candidates(word, &cands));
            let pairs = matches
                .into_iter()
                .map(|c| Candidate {
                    display: c.clone(),
                    replacement: format!("{c} "),
                })
                .collect();
            (word_start, pairs)
        }
        CompleteKind::File => {
            // Split the word into (dir, base) around the last path separator.
            let sep = cake_platform::get().path_separator();
            let (dir, base, dir_prefix) =
                match word.rfind(|c| cake_platform::get().is_path_separator(c)) {
                    Some(0) => (sep.to_string(), &word[1..], sep.to_string()),
                    Some(i) => (
                        word[..i].to_string(),
                        &word[i + 1..],
                        word[..=i].to_string(),
                    ),
                    None => (String::from("."), word, String::new()),
                };
            let mut cands: Vec<String> = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let full = e.path().to_string_lossy().into_owned();
                    if cake_platform::get().stat(&full).is_dir {
                        cands.push(format!("{name}{sep}"));
                    } else {
                        cands.push(name);
                    }
                }
            }
            let matches = dedup(cake_complete::filter_candidates(base, &cands));
            let pairs = matches
                .into_iter()
                .map(|c| Candidate {
                    display: c.clone(),
                    replacement: format!("{dir_prefix}{c}"),
                })
                .collect();
            (word_start, pairs)
        }
        CompleteKind::Variable => {
            let dollar = cake_complete::dollar_in_word(word).unwrap_or(0);
            let prefix = word[dollar + 1..].trim_end_matches(['"', '\'']);
            let names: Vec<String> = exec.env.get_names().iter().map(|n| n.to_string()).collect();
            let matches = dedup(cake_complete::filter_candidates(prefix, &names));
            let pairs = matches
                .into_iter()
                .map(|c| Candidate {
                    display: format!("${c}"),
                    replacement: c,
                })
                .collect();
            // Keep the leading `$` (and anything before it) in the line;
            // replace only the name part.
            (word_start + dollar + 1, pairs)
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

/// Run one line-edit session for `prompt` (PS1 or PS2), wiring the editor to
/// the executor for highlighting, autosuggestion and completion. `parser` and
/// `pending` persist across sessions so batched input (pasted lines) is not
/// lost.
#[allow(clippy::type_complexity)]
fn edit_line(
    editor: &mut LineEditor,
    parser: &mut KeyParser,
    pending: &mut Vec<Key>,
    executor: &Rc<RefCell<Executor>>,
    prompt: &str,
    right: &str,
) -> Result<ReadOutcome, String> {
    let highlight = |s: &str| {
        let exec = executor.borrow();
        let found = |name: &str| {
            exec.aliases.contains_key(name)
                || !matches!(
                    cake_exec::resolve::resolve_command(&exec, name),
                    cake_exec::resolve::CommandSpec::NotFound
                )
        };
        cake_highlight::highlight_line(s, &found)
    };
    let hint = |line: &str, hist: &[String]| {
        let exec = executor.borrow();
        // Skip history entries whose leading command is known-bad.
        let filtered: Vec<String> = hist
            .iter()
            .filter(|entry| {
                let first = entry.split_whitespace().next().unwrap_or("");
                !exec.blacklist.contains(first)
            })
            .cloned()
            .collect();
        cake_reader::suggest(line, &filtered)
    };
    let complete = |line: &str, pos: usize| -> Option<(usize, Vec<Candidate>)> {
        let exec = executor.borrow();
        Some(complete_in(&exec, line, pos))
    };
    read_loop(
        editor, parser, pending, prompt, right, &highlight, &hint, &complete,
    )
}

/// Run the interactive loop. Never returns.
pub fn run_interactive() -> ! {
    let executor = Rc::new(RefCell::new(Executor::new(import_env())));
    {
        let mut exec = executor.borrow_mut();
        exec.shell_pid = std::process::id() as i32;
        exec.interactive = true;
        // Aliases expand in interactive shells (bash expands them in `-c`
        // scripts only when `shopt -s expand_aliases` is set, which cake
        // doesn't do).
        exec.expand_aliases = true;
        load_blacklist(&mut exec);
        load_rc(&mut exec);
    }

    // When stdin is not a terminal (e.g. `echo 'ls' | cake`), read plain
    // lines instead of using the raw-mode line editor.
    if !std::io::stdin().is_terminal() {
        let mut exec = executor.borrow_mut();
        run_piped(&mut exec);
    }

    let mut editor = LineEditor::new();
    if let Ok(text) = std::fs::read_to_string(history_path()) {
        let entries = text.lines().map(|l| l.to_string()).collect::<Vec<String>>();
        editor.history_mut().set_entries(entries);
    }
    let mut parser = KeyParser::new();
    let mut pending: Vec<Key> = Vec::new();

    let mut buffer = String::new();
    let mut last_elapsed_ms: Option<u64> = None;
    loop {
        buffer.clear();
        let (mut prompt, mut right) = {
            let exec = executor.borrow();
            compute_prompt(&exec, last_elapsed_ms)
        };
        loop {
            match edit_line(
                &mut editor,
                &mut parser,
                &mut pending,
                &executor,
                &prompt,
                &right,
            ) {
                Ok(ReadOutcome::Line(line)) => {
                    buffer.push_str(&line);
                    buffer.push('\n');
                }
                Ok(ReadOutcome::Eof) => {
                    if buffer.is_empty() {
                        println!();
                        save_history(&editor);
                        let mut exec = executor.borrow_mut();
                        exec.run_exit_traps();
                        save_blacklist(&exec);
                        std::process::exit(0);
                    }
                    // Ctrl-D during a continuation discards the input.
                    buffer.clear();
                    (prompt, right) = {
                        let exec = executor.borrow();
                        compute_prompt(&exec, last_elapsed_ms)
                    };
                    continue;
                }
                Ok(ReadOutcome::Interrupted) => {
                    // Ctrl-C: cancel the current line(s), start over.
                    println!();
                    buffer.clear();
                    (prompt, right) = {
                        let exec = executor.borrow();
                        compute_prompt(&exec, last_elapsed_ms)
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
                    // Record the executed line for autosuggestion.
                    let trimmed = buffer.trim().to_string();
                    if !trimmed.is_empty() {
                        editor.history_mut().push(trimmed);
                    }
                    let start = cake_platform::get().time_nanos();
                    let outcome = {
                        let mut exec = executor.borrow_mut();
                        exec.eval_str(&buffer)
                    };
                    last_elapsed_ms =
                        Some(cake_platform::get().time_nanos().saturating_sub(start) / 1_000_000);
                    if let Some(err) = outcome.error {
                        eprintln!("{err}");
                    }
                    let code = {
                        let mut exec = executor.borrow_mut();
                        exec.take_exit_requested()
                    };
                    if let Some(code) = code {
                        save_history(&editor);
                        let mut exec = executor.borrow_mut();
                        exec.run_exit_traps();
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
            executor.run_exit_traps();
            save_blacklist(executor);
            std::process::exit(code);
        }
    }
    executor.run_exit_traps();
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

/// Choose between the segmented p10k-style prompt and the legacy PS1 prompt.
/// Returns `(prompt, right_prompt)`; the legacy path has an empty right side.
fn compute_prompt(exec: &Executor, last_elapsed_ms: Option<u64>) -> (String, String) {
    if prompt_enabled(exec) {
        build_prompt(exec, last_elapsed_ms)
    } else {
        (prompt1(exec), String::new())
    }
}

/// The primary prompt: an explicit `PS1` wins, otherwise the current path
/// (abbreviated to `~` under `$HOME`), falling back to `$ ` without `PWD`.
fn prompt1(exec: &Executor) -> String {
    if let Some(ps1) = exec.env.get("PS1").filter(|v| !v.value().is_empty()) {
        return ps1.value().to_owned();
    }
    match exec.env.get("PWD") {
        Some(pwd) if !pwd.value().is_empty() => {
            let pwd = pwd.value();
            match exec.env.get("HOME") {
                Some(home) if !home.value().is_empty() => {
                    if let Some(rest) = pwd.strip_prefix(home.value()) {
                        return if rest.is_empty() {
                            "~ ".to_owned()
                        } else {
                            format!("~{rest} ")
                        };
                    }
                }
                _ => {}
            }
            format!("{pwd} ")
        }
        _ => "$ ".to_owned(),
    }
}

fn data_dir() -> PathBuf {
    PathBuf::from(cake_platform::get().xdg_dir(XdgKind::Data)).join("cake")
}

fn history_path() -> PathBuf {
    data_dir().join("history")
}

fn save_history(editor: &LineEditor) {
    let path = history_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = editor.history().entries().join("\n");
    let _ = std::fs::write(path, text);
}

pub(crate) fn load_blacklist(exec: &mut Executor) {
    let path = data_dir().join("blacklist");
    if let Ok(text) = std::fs::read_to_string(path) {
        exec.blacklist = CommandBlacklist::from_text(&text);
    }
}

/// Evaluate the startup rc file (`$XDG_DATA_HOME/cake/rc`) if it exists.
/// Aliases and prompt settings set there persist for the session.
pub(crate) fn load_rc(exec: &mut Executor) {
    let path = data_dir().join("rc");
    if let Ok(text) = std::fs::read_to_string(path) {
        let _ = exec.eval_str(&text);
    }
}

pub(crate) fn save_blacklist(exec: &Executor) {
    let path = data_dir().join("blacklist");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, exec.blacklist.to_text());
}
