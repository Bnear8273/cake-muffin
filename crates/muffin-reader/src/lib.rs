//! Interactive reader logic (pure, `#![no_std]`).
//!
//! Currently provides context-aware autosuggestion: given the line being typed
//! and the command history, propose a suffix for the rest of the line.
//! Line-editing itself lives in the std driver (`muffin-editor`).

#![no_std]

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::string::String;

/// Suggest a suffix (dim text shown after the cursor) for `line` based on
/// the most recent matching history entry.
///
/// Returns `None` when there is no strictly-longer matching history line.
pub fn suggest(line: &str, history: &[String]) -> Option<String> {
    let line = line.trim_end();
    if line.is_empty() {
        return None;
    }
    for entry in history.iter().rev() {
        let entry = entry.trim_end();
        if entry.len() > line.len() && entry.starts_with(line) {
            return Some(entry[line.len()..].to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec::Vec;

    fn h(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn suggests_from_history() {
        let history = h(&["echo hello", "ls /tmp", "cd /etc"]);
        assert_eq!(suggest("echo he", &history), Some("llo".into()));
        assert_eq!(suggest("ls /tm", &history), Some("p".into()));
    }

    #[test]
    fn picks_newest_matching() {
        let history = h(&["git status", "echo a", "git log"]);
        assert_eq!(suggest("git", &history), Some(" log".into()));
    }

    #[test]
    fn no_suggestion_for_empty_or_exact() {
        let history = h(&["echo hello"]);
        assert_eq!(suggest("", &history), None);
        assert_eq!(suggest("echo hello", &history), None);
    }
}
