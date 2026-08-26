//! Command resolution and introspection.

use alloc::string::String;

use crate::builtins::is_builtin;
use crate::executor::Executor;
use crate::path::find_in_path;

/// Describe what `name` resolves to, for `type`.
pub fn describe(exec: &Executor, name: &str) -> String {
    if is_builtin(name) {
        return "a shell builtin".into();
    }
    if exec.functions.contains_key(name) {
        return "a function".into();
    }
    if let Some(path) = find_in_path(&exec.env, name) {
        return alloc::format!("{path}");
    }
    alloc::format!("cake: not found")
}

/// Look up a command name: builtin, function, or external path.
#[derive(Debug, Clone)]
pub enum CommandSpec {
    Builtin,
    Function,
    External(String),
    NotFound,
}

/// Resolve a command word to its kind.
pub fn resolve_command(exec: &Executor, name: &str) -> CommandSpec {
    if is_builtin(name) {
        return CommandSpec::Builtin;
    }
    if exec.functions.contains_key(name) {
        return CommandSpec::Function;
    }
    match find_in_path(&exec.env, name) {
        Some(path) => CommandSpec::External(path),
        None => CommandSpec::NotFound,
    }
}
