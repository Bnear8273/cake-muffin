//! Persistent blacklist of "command not found" commands.
//!
//! When a bare command name fails to resolve (no builtin, function, alias, or
//! PATH entry), it is recorded here so the completion/autosuggestion layers
//! can stop suggesting it. When the command later resolves successfully, it
//! is removed again.
//!
//! The blacklist only affects *suggestions* — it never blocks execution.
//!
//! This crate holds only the in-memory set and its text serialization. File
//! persistence lives in the shell driver (the std binary), which reads the
//! file into a [`CommandBlacklist`] at startup and writes it back on change.

#![no_std]

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::collections::BTreeSet;
use alloc::string::String;

/// Whether `name` looks path-specified (`/` on Unix, `\` on Windows, or a
/// drive-qualified name) and therefore must not be blacklisted.
fn has_path_separator(name: &str) -> bool {
    name.contains('/') || name.contains('\\')
}

/// A set of blacklisted command names.
///
/// Never contains names with a path separator (only bare command names are
/// blacklisted; path-specified commands resolve via file completion).
#[derive(Debug, Clone, Default)]
pub struct CommandBlacklist {
    set: BTreeSet<String>,
}

impl CommandBlacklist {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from a newline-delimited text blob (the on-disk format).
    pub fn from_text(text: &str) -> Self {
        let mut set = BTreeSet::new();
        for line in text.lines() {
            let name = line.trim();
            if !name.is_empty() && !has_path_separator(name) {
                set.insert(name.to_owned());
            }
        }
        Self { set }
    }

    /// Serialize to a newline-delimited text blob.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for name in &self.set {
            out.push_str(name);
            out.push('\n');
        }
        out
    }

    /// Whether `name` is blacklisted.
    pub fn contains(&self, name: &str) -> bool {
        !has_path_separator(name) && self.set.contains(name)
    }

    /// Add a command name to the blacklist.
    pub fn insert(&mut self, name: &str) {
        if !has_path_separator(name) {
            self.set.insert(name.to_owned());
        }
    }

    /// Remove a command name from the blacklist.
    pub fn remove(&mut self, name: &str) {
        self.set.remove(name);
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.set.clear();
    }

    /// All blacklisted names, sorted.
    pub fn names(&self) -> alloc::vec::Vec<&str> {
        self.set.iter().map(String::as_str).collect()
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_text() {
        let mut bl = CommandBlacklist::new();
        bl.insert("clar");
        bl.insert("nonexistent");
        let text = bl.to_text();

        let bl2 = CommandBlacklist::from_text(&text);
        assert!(bl2.contains("clar"));
        assert!(bl2.contains("nonexistent"));
        assert!(!bl2.contains("clear"));
    }

    #[test]
    fn ignores_slashed_names() {
        let mut bl = CommandBlacklist::new();
        bl.insert("ls");
        bl.insert("./script.sh");
        assert!(bl.contains("ls"));
        assert!(!bl.contains("./script.sh"));
    }

    #[test]
    fn remove_clear() {
        let mut bl = CommandBlacklist::new();
        bl.insert("foo");
        assert!(bl.contains("foo"));
        bl.remove("foo");
        assert!(!bl.contains("foo"));
    }
}