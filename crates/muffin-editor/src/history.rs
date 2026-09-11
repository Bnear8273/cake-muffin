//! Command history with up/down navigation. The currently edited line is kept
//! as a "draft" so that pressing Down back to the end restores what was typed.

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

/// A bounded, deduplicating command history.
pub struct History {
    entries: Vec<String>,
    cap: usize,
    /// Index into `entries` during up/down navigation, or `None` at the live
    /// editing position.
    pos: Option<usize>,
    /// The line that was being edited when navigation started.
    draft: Option<String>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl History {
    pub fn new(cap: usize) -> Self {
        History {
            entries: Vec::new(),
            cap,
            pos: None,
            draft: None,
        }
    }

    /// Append a completed command line. Empty/whitespace lines and exact
    /// repeats of the previous entry are skipped.
    pub fn push(&mut self, line: String) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        let entry = trimmed.to_owned();
        if self.entries.last() == Some(&entry) {
            return;
        }
        if self.entries.len() == self.cap {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    /// Move to the previous entry. The first move stashes the current line as
    /// the draft; every subsequent move walks further back. Returns the line
    /// to display, or `None` if there is no history.
    pub fn prev(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        match self.pos {
            None => {
                self.draft = Some(current.to_owned());
                self.pos = Some(self.entries.len() - 1);
            }
            Some(p) => {
                if p == 0 {
                    // Already at the oldest entry: stay put.
                    return Some(self.entries[0].clone());
                }
                self.pos = Some(p - 1);
            }
        }
        Some(self.entries[self.pos.unwrap()].clone())
    }

    /// Move to the next (newer) entry, restoring the draft once past the
    /// newest.
    pub fn forward(&mut self) -> Option<String> {
        let p = self.pos?;
        if p + 1 >= self.entries.len() {
            self.pos = None;
            return self.draft.take();
        }
        self.pos = Some(p + 1);
        Some(self.entries[p + 1].clone())
    }

    /// Leave navigation and drop the draft.
    pub fn reset_nav(&mut self) {
        self.pos = None;
        self.draft = None;
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Replace the whole history (e.g. loaded from disk at startup).
    /// Comment lines (leading `#`, the rustyline `#V2` header) are dropped.
    pub fn set_entries(&mut self, v: Vec<String>) {
        self.entries.clear();
        for line in v {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            self.entries.push(trimmed.to_owned());
        }
        let overflow = self.entries.len().saturating_sub(self.cap);
        if overflow > 0 {
            self.entries.drain(..overflow);
        }
        self.reset_nav();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.reset_nav();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn push_skips_empty_and_repeats() {
        let mut h = History::new(1000);
        h.push("ls".into());
        h.push("".into());
        h.push("   ".into());
        h.push("ls".into());
        h.push("pwd".into());
        assert_eq!(h.entries(), &["ls", "pwd"]);
    }

    #[test]
    fn push_truncates_to_cap() {
        let mut h = History::new(3);
        h.push("a".into());
        h.push("b".into());
        h.push("c".into());
        h.push("d".into());
        assert_eq!(h.entries(), &["b", "c", "d"]);
    }

    #[test]
    fn prev_walks_back_with_draft() {
        let mut h = History::new(1000);
        h.push("one".into());
        h.push("two".into());
        h.push("three".into());
        // First Up stashes the current draft.
        assert_eq!(h.prev("draft"), Some("three".into()));
        assert_eq!(h.prev("three"), Some("two".into()));
        assert_eq!(h.prev("two"), Some("one".into()));
        // At the oldest entry, stay.
        assert_eq!(h.prev("one"), Some("one".into()));
    }

    #[test]
    fn next_restores_draft() {
        let mut h = History::new(1000);
        h.push("one".into());
        h.push("two".into());
        assert_eq!(h.prev("draft"), Some("two".into()));
        assert_eq!(h.forward(), Some("draft".into()));
        // After the draft is restored, further Down does nothing.
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn next_walks_newer() {
        let mut h = History::new(1000);
        h.push("one".into());
        h.push("two".into());
        h.push("three".into());
        assert_eq!(h.prev("draft"), Some("three".into()));
        assert_eq!(h.prev("three"), Some("two".into()));
        assert_eq!(h.forward(), Some("three".into()));
        assert_eq!(h.forward(), Some("draft".into()));
    }

    #[test]
    fn empty_history() {
        let mut h = History::new(1000);
        assert_eq!(h.prev("x"), None);
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn reset_nav_clears_draft() {
        let mut h = History::new(1000);
        h.push("one".into());
        h.prev("draft");
        h.reset_nav();
        assert_eq!(h.forward(), None);
        assert_eq!(h.prev("draft"), Some("one".into()));
    }

    #[test]
    fn set_entries_skips_comments() {
        let mut h = History::new(1000);
        h.set_entries(vec![
            "#V2".into(),
            "ls".into(),
            "# rustyline header".into(),
            "pwd".into(),
        ]);
        assert_eq!(h.entries(), &["ls", "pwd"]);
    }

    #[test]
    fn set_entries_trims_and_caps() {
        let mut h = History::new(2);
        h.set_entries(vec!["  a  ".into(), "".into(), "b".into(), "c".into()]);
        assert_eq!(h.entries(), &["b", "c"]);
        assert_eq!(h.forward(), None);
    }
}
