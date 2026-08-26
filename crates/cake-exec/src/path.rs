//! Command resolution: turn a command word into an executable path.
//!
//! Pure logic: the platform's `is_executable` bridges to the OS.

use alloc::borrow::ToOwned;
use alloc::string::String;

use cake_env::EnvStack;

/// Resolve a command word to an absolute executable path.
///
/// Mirrors bash:
/// * a word containing `/` is used as-is (must be an existing executable);
/// * otherwise `PATH` is searched, respecting an empty entry as "current
///   directory" (which bash treats as skipped in non-privileged shells);
/// * returns `None` if nothing matched (the caller reports "command not
///   found").
pub fn find_in_path(env: &EnvStack, cmd: &str) -> Option<String> {
    let platform = cake_platform::get();

    if cmd.contains('/') {
        return if platform.is_executable(cmd) {
            Some(cmd.to_owned())
        } else {
            None
        };
    }

    let path_var = env.get("PATH")?;
    for dir in path_var.values() {
        if dir.is_empty() {
            continue;
        }
        let p = if dir.ends_with('/') {
            alloc::format!("{dir}{cmd}")
        } else {
            alloc::format!("{dir}/{cmd}")
        };
        if platform.is_executable(&p) {
            return Some(p);
        }
    }
    None
}