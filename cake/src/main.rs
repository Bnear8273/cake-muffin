//! The cake-shell driver binary.
//!
//! This is the only crate that links the OS layer (`cake-platform-unix`) into
//! the pure no_std shell crates. It owns all std-only concerns: importing the
//! process environment, picking the platform backend, and printing errors.

use cake_env::{EnvStack, EnvVar, EnvVarFlags};
use cake_exec::Executor;
use cake_platform::init as platform_init;
use cake_platform_unix::unix_backend;

mod repl;

fn main() {
    // 1. Install the platform backend (must happen before any shell work).
    platform_init(unix_backend());

    // 2. Parse args.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        ParsedArgs::Help => {
            print_help();
            std::process::exit(0);
        }
        ParsedArgs::Error(msg) => {
            eprintln!("cake: {msg}");
            eprintln!("Try 'cake --help' for more information.");
            std::process::exit(2);
        }
        ParsedArgs::Command(cmd, rest) => {
            let env = import_env();
            let mut executor = Executor::new(env);
            executor.shell_pid = std::process::id() as i32;
            crate::repl::load_blacklist(&mut executor);
            // Like `bash -c CMD arg...`: the first trailing arg is `$0`,
            // the rest are `$1..`; if none are given, `$0` is the shell.
            if rest.is_empty() {
                executor.positional = vec!["cake".into()];
            } else {
                executor.positional = rest;
            }
            let outcome = executor.eval_str(&cmd);
            if let Some(err) = outcome.error {
                eprintln!("{err}");
            }
            crate::repl::save_blacklist(&executor);
            executor.run_exit_traps();
            std::process::exit(outcome.status.status_code());
        }
        ParsedArgs::Interactive => {
            crate::repl::run_interactive();
        }
    }
}

/// Seed the shell environment from the process environment.
///
/// `PATH` is split into a path-list variable using the platform's separator
/// convention; everything else becomes a single-valued exported variable.
fn import_env() -> EnvStack {
    let mut stack = EnvStack::new();
    let vars: Vec<(String, EnvVar)> = std::env::vars()
        .map(|(name, value)| {
            if name == "PATH" {
                let list: Vec<String> = std::env::split_paths(&value)
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                (
                    name,
                    EnvVar::from_path(list)
                        .set_flags(EnvVarFlags::EXPORT | EnvVarFlags::PATHVAR),
                )
            } else {
                (name, EnvVar::new(value).set_flags(EnvVarFlags::EXPORT))
            }
        })
        .collect();
    stack.seed_globals(vars);
    stack
}

enum ParsedArgs {
    /// `-c CMD` plus trailing args (which become positional parameters).
    Command(String, Vec<String>),
    Help,
    Error(String),
    Interactive,
}

/// Argument parsing: `cake -c "command" args...`, `cake --help`, or plain
/// `cake` for interactive mode.
fn parse_args(args: &[String]) -> ParsedArgs {
    if args.is_empty() {
        return ParsedArgs::Interactive;
    }
    match args[0].as_str() {
        "-c" | "--command" => {
            if args.len() < 2 {
                return ParsedArgs::Error("option requires an argument: -c".into());
            }
            ParsedArgs::Command(args[1].clone(), args[2..].to_vec())
        }
        "-h" | "--help" => ParsedArgs::Help,
        other => ParsedArgs::Error(format!("unexpected argument '{other}'")),
    }
}

fn print_help() {
    println!("cake - a bash-compatible shell with a modern interactive layer");
    println!();
    println!("Usage: cake [OPTIONS] [ARG...]");
    println!("       cake -c 'COMMAND' [ARG...]");
    println!();
    println!("With no arguments, cake starts an interactive shell.");
    println!();
    println!("Options:");
    println!("  -c, --command <CMD>  run the command string and exit; trailing");
    println!("                       args become positional parameters");
    println!("  -h, --help           print this help");
    println!();
    println!("Builtins:");
    let names = cake_exec::builtins::builtin_names();
    for chunk in names.chunks(5) {
        println!("  {}", chunk.join(" "));
    }
    println!();
    println!("Features:");
    println!("  control flow     if/elif/else, for, while, until, case, {{ }} blocks, ( ) subshells, functions");
    println!("  pipes            |, &&, ||, !, & (background)");
    println!("  redirections     >, >>, <, 2>, 2>&1, &>, &>>, heredocs, herestrings");
    println!("  expansion        $var, ${{var}}, $?, $#, $@, $*, $$, $((arith)), ~, quotes, IFS splitting");
    println!("  conditionals     [ ... ] and [[ ... ]] tests, arithmetic (( ... ))");
}
