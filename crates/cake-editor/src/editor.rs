//! The line-editing state machine. Holds the editing buffer (a `String` with a
//! byte-offset cursor that never splits a codepoint), the history, the cached
//! autosuggestion hint, and the two-tap completion state.

use alloc::string::String;

use crate::complete::{CompletionState, longest_common_prefix};
use crate::keys::{CtrlKey, Key};
use crate::render::{Render, compute_render};
use crate::{EditEvent, History, Services};

/// The editing session for one `readline` call.
pub struct Editor {
    buffer: String,
    /// Byte offset of the cursor; always on a char boundary.
    cursor: usize,
    history: History,
    /// Cached autosuggestion suffix, recomputed after each edit.
    hint: Option<String>,
    completion: Option<CompletionState>,
    /// Set by Ctrl-L; the driver consumes it via `take_clear_screen`.
    clear_screen: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        Editor {
            buffer: String::new(),
            cursor: 0,
            history: History::new(1000),
            hint: None,
            completion: None,
            clear_screen: false,
        }
    }

    /// Clear the line and completion state for a fresh `readline` call. The
    /// history is kept across calls.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.hint = None;
        self.completion = None;
        self.clear_screen = false;
        self.history.reset_nav();
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn cursor_byte(&self) -> usize {
        self.cursor
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// Consume the clear-screen flag set by Ctrl-L.
    pub fn take_clear_screen(&mut self) -> bool {
        let c = self.clear_screen;
        self.clear_screen = false;
        c
    }

    /// Apply one key press, consulting the outside world through `srv`.
    pub fn handle(&mut self, key: Key, srv: &Services<'_>) -> EditEvent {
        match key {
            Key::Char(c) => {
                self.insert_char(c);
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Enter => EditEvent::Submit(self.buffer.clone()),
            Key::Backspace => {
                self.backspace();
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Delete => {
                self.delete_char();
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Left | Key::Ctrl(CtrlKey::B) => {
                self.move_left();
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Right | Key::Ctrl(CtrlKey::F) => {
                if self.accept_hint() {
                    self.after_edit(srv);
                    return EditEvent::Redraw;
                }
                self.move_right();
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Home | Key::Ctrl(CtrlKey::A) => {
                self.cursor = 0;
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::End | Key::Ctrl(CtrlKey::E) => {
                if !self.accept_hint() {
                    self.cursor = self.buffer.len();
                }
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Up => {
                if let Some(line) = self.history.prev(&self.buffer) {
                    self.set_line(line);
                }
                EditEvent::Redraw
            }
            Key::Down => {
                if let Some(line) = self.history.forward() {
                    self.set_line(line);
                }
                EditEvent::Redraw
            }
            Key::Ctrl(CtrlKey::K) => {
                self.buffer.truncate(self.cursor);
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Ctrl(CtrlKey::U) => {
                self.buffer.drain(..self.cursor);
                self.cursor = 0;
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Ctrl(CtrlKey::W) => {
                self.kill_prev_word();
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Ctrl(CtrlKey::L) => {
                self.clear_screen = true;
                self.after_edit(srv);
                EditEvent::Redraw
            }
            Key::Ctrl(CtrlKey::C) => {
                self.buffer.clear();
                self.cursor = 0;
                self.hint = None;
                self.completion = None;
                self.history.reset_nav();
                EditEvent::Interrupted
            }
            Key::Ctrl(CtrlKey::D) => {
                if self.buffer.is_empty() {
                    EditEvent::Eof
                } else {
                    self.delete_char();
                    self.after_edit(srv);
                    EditEvent::Redraw
                }
            }
            Key::Tab => self.tab(srv),
            Key::BackTab => EditEvent::NoChange,
            Key::Esc => {
                self.completion = None;
                EditEvent::Redraw
            }
        }
    }

    /// Derive the frame the driver should paint for the current state.
    pub fn render(&self, srv: &Services<'_>) -> Render {
        let completion = self
            .completion
            .as_ref()
            .filter(|cs| cs.list_shown)
            .map(|cs| cs.candidates.as_slice());
        compute_render(
            srv.prompt,
            srv.right,
            &self.buffer,
            self.cursor,
            self.hint.as_deref(),
            completion,
            srv.width,
            srv.highlight,
        )
    }

    fn after_edit(&mut self, srv: &Services<'_>) {
        self.completion = None;
        self.history.reset_nav();
        self.refresh_hint(srv);
    }

    fn refresh_hint(&mut self, srv: &Services<'_>) {
        if self.cursor == self.buffer.len() && !self.buffer.is_empty() {
            self.hint = (srv.hint)(&self.buffer, self.history.entries());
        } else {
            self.hint = None;
        }
    }

    /// Load a line (from history navigation): cursor at end, no hint.
    fn set_line(&mut self, line: String) {
        self.buffer = line;
        self.cursor = self.buffer.len();
        self.hint = None;
        self.completion = None;
    }

    /// At end-of-line with a pending hint, append it and return true.
    fn accept_hint(&mut self) -> bool {
        if self.cursor != self.buffer.len() {
            return false;
        }
        let Some(h) = self.hint.take() else {
            return false;
        };
        self.buffer.push_str(&h);
        self.cursor = self.buffer.len();
        true
    }

    fn tab(&mut self, srv: &Services<'_>) -> EditEvent {
        // Second (or later) Tab with a pending completion.
        if let Some(cs) = &mut self.completion {
            if cs.list_shown {
                return EditEvent::NoChange;
            }
            cs.list_shown = true;
            return EditEvent::Redraw;
        }

        let Some((start, cands)) = (srv.complete)(&self.buffer, self.cursor) else {
            return EditEvent::NoChange;
        };
        if cands.is_empty() {
            return EditEvent::NoChange;
        }
        if cands.len() == 1 {
            let rep = cands[0].replacement.clone();
            self.buffer.replace_range(start..self.cursor, &rep);
            self.cursor = start + rep.len();
            self.after_edit(srv);
            return EditEvent::Redraw;
        }
        // Multiple candidates: splice the longest common prefix.
        let lcp = longest_common_prefix(cands.iter().map(|c| c.replacement.as_str()));
        let current_len = self.buffer[start..self.cursor].len();
        if lcp.len() > current_len {
            self.buffer.replace_range(start..self.cursor, &lcp);
            self.cursor = start + lcp.len();
        }
        self.completion = Some(CompletionState {
            start,
            candidates: cands,
            list_shown: false,
        });
        self.refresh_hint(srv);
        EditEvent::Redraw
    }

    fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor].chars().next_back().unwrap();
        let start = self.cursor - prev.len_utf8();
        self.buffer.drain(start..self.cursor);
        self.cursor = start;
    }

    fn delete_char(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let c = self.buffer[self.cursor..].chars().next().unwrap();
        self.buffer.drain(self.cursor..self.cursor + c.len_utf8());
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor].chars().next_back().unwrap();
        self.cursor -= prev.len_utf8();
    }

    fn move_right(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let c = self.buffer[self.cursor..].chars().next().unwrap();
        self.cursor += c.len_utf8();
    }

    fn kill_prev_word(&mut self) {
        let bytes = self.buffer.as_bytes();
        let mut i = self.cursor;
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        while i > 0 && !bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        self.buffer.drain(i..self.cursor);
        self.cursor = i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Candidate;
    use crate::Services;
    use alloc::boxed::Box;
    use alloc::vec;

    fn srv_with<C>(complete: C) -> Services<'static>
    where
        C: Fn(&str, usize) -> Option<(usize, alloc::vec::Vec<Candidate>)> + 'static,
    {
        // Box::leak keeps the (non-capturing) closure alive for 'static; each
        // test leaks one tiny closure, which is fine for tests.
        let c: &'static C = Box::leak(Box::new(complete));
        Services {
            prompt: "$ ",
            right: "",
            width: 80,
            highlight: &(|s| String::from(s)),
            hint: &(|line, _| {
                if line == "ec" {
                    Some("ho".into())
                } else {
                    None
                }
            }),
            complete: c,
        }
    }

    fn srv() -> Services<'static> {
        srv_with(|_, _| None)
    }

    fn type_line(ed: &mut Editor, srv: &Services<'_>, text: &str) {
        for c in text.chars() {
            ed.handle(Key::Char(c), srv);
        }
    }

    #[test]
    fn insert_and_cursor() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "hello");
        assert_eq!(ed.buffer(), "hello");
        assert_eq!(ed.cursor_byte(), 5);
    }

    #[test]
    fn insert_in_middle() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "hllo");
        ed.handle(Key::Left, &srv);
        ed.handle(Key::Left, &srv);
        ed.handle(Key::Left, &srv);
        ed.handle(Key::Char('e'), &srv);
        assert_eq!(ed.buffer(), "hello");
        assert_eq!(ed.cursor_byte(), 2);
    }

    #[test]
    fn utf8_never_splits_codepoint() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "a你好é");
        assert_eq!(ed.cursor_byte(), ed.buffer().len());
        // Move left over each char; the char at the cursor is the full
        // codepoint that was just revealed.
        ed.handle(Key::Left, &srv);
        assert!(ed.buffer().is_char_boundary(ed.cursor_byte()));
        assert_eq!(ed.buffer()[ed.cursor_byte()..].chars().next(), Some('é'));
        ed.handle(Key::Left, &srv);
        assert_eq!(ed.buffer()[ed.cursor_byte()..].chars().next(), Some('好'));
        // Backspace the 你.
        ed.handle(Key::Backspace, &srv);
        assert_eq!(ed.buffer(), "a好é");
        // Delete the 好.
        ed.handle(Key::Delete, &srv);
        assert_eq!(ed.buffer(), "aé");
    }

    #[test]
    fn delete_and_backspace_at_edges() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "ab");
        ed.handle(Key::Home, &srv);
        ed.handle(Key::Delete, &srv);
        assert_eq!(ed.buffer(), "b");
        ed.handle(Key::Backspace, &srv); // no-op at start
        assert_eq!(ed.buffer(), "b");
        ed.handle(Key::End, &srv);
        ed.handle(Key::Backspace, &srv);
        assert_eq!(ed.buffer(), "");
        ed.handle(Key::Delete, &srv); // no-op at end
        assert_eq!(ed.buffer(), "");
    }

    #[test]
    fn ctrl_k_u_w() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "hello world foo");
        ed.handle(Key::Ctrl(CtrlKey::A), &srv);
        ed.handle(Key::Ctrl(CtrlKey::K), &srv);
        assert_eq!(ed.buffer(), "");

        type_line(&mut ed, &srv, "hello world");
        ed.handle(Key::Ctrl(CtrlKey::U), &srv);
        assert_eq!(ed.buffer(), "");

        type_line(&mut ed, &srv, "echo hello");
        ed.handle(Key::Ctrl(CtrlKey::W), &srv);
        assert_eq!(ed.buffer(), "echo ");
        ed.handle(Key::Ctrl(CtrlKey::W), &srv);
        assert_eq!(ed.buffer(), "");
    }

    #[test]
    fn ctrl_d_eof_and_delete() {
        let mut ed = Editor::new();
        let srv = srv();
        assert_eq!(ed.handle(Key::Ctrl(CtrlKey::D), &srv), EditEvent::Eof);
        type_line(&mut ed, &srv, "abc");
        ed.handle(Key::Home, &srv);
        assert_eq!(ed.handle(Key::Ctrl(CtrlKey::D), &srv), EditEvent::Redraw);
        assert_eq!(ed.buffer(), "bc");
    }

    #[test]
    fn ctrl_c_interrupts() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "half");
        assert_eq!(
            ed.handle(Key::Ctrl(CtrlKey::C), &srv),
            EditEvent::Interrupted
        );
        assert_eq!(ed.buffer(), "");
    }

    #[test]
    fn enter_submits() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "ls -l");
        assert_eq!(
            ed.handle(Key::Enter, &srv),
            EditEvent::Submit("ls -l".into())
        );
    }

    #[test]
    fn hint_shown_at_eol_only() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "ec");
        assert_eq!(ed.hint.as_deref(), Some("ho"));
        // Moving off the end hides it.
        ed.handle(Key::Left, &srv);
        assert_eq!(ed.hint, None);
        // Moving back to the end recomputes it.
        ed.handle(Key::Right, &srv);
        assert_eq!(ed.hint.as_deref(), Some("ho"));
    }

    #[test]
    fn right_accepts_hint() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "ec");
        ed.handle(Key::Right, &srv);
        assert_eq!(ed.buffer(), "echo");
        assert_eq!(ed.hint, None);
    }

    #[test]
    fn end_accepts_hint() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "ec");
        ed.handle(Key::End, &srv);
        assert_eq!(ed.buffer(), "echo");
    }

    #[test]
    fn history_navigation_with_draft() {
        let mut ed = Editor::new();
        let srv = srv();
        ed.history_mut().push("ls".into());
        ed.history_mut().push("pwd".into());
        type_line(&mut ed, &srv, "draft");
        ed.handle(Key::Up, &srv);
        assert_eq!(ed.buffer(), "pwd");
        ed.handle(Key::Up, &srv);
        assert_eq!(ed.buffer(), "ls");
        ed.handle(Key::Down, &srv);
        assert_eq!(ed.buffer(), "pwd");
        ed.handle(Key::Down, &srv);
        assert_eq!(ed.buffer(), "draft");
    }

    #[test]
    fn typing_resets_history_nav() {
        let mut ed = Editor::new();
        let srv = srv();
        ed.history_mut().push("ls".into());
        type_line(&mut ed, &srv, "x");
        ed.handle(Key::Up, &srv);
        assert_eq!(ed.buffer(), "ls");
        ed.handle(Key::Char('!'), &srv);
        assert_eq!(ed.buffer(), "ls!");
        // Down now leaves navigation (nav was reset).
        ed.handle(Key::Down, &srv);
        assert_eq!(ed.buffer(), "ls!");
    }

    #[test]
    fn tab_single_candidate_splices() {
        let mut ed = Editor::new();
        let srv = srv_with(|line, pos| {
            assert_eq!((line, pos), ("l", 1));
            Some((
                0,
                vec![Candidate {
                    display: "ls".into(),
                    replacement: "ls ".into(),
                }],
            ))
        });
        type_line(&mut ed, &srv, "l");
        assert_eq!(ed.handle(Key::Tab, &srv), EditEvent::Redraw);
        assert_eq!(ed.buffer(), "ls ");
    }

    #[test]
    fn tab_lcp_then_list() {
        let mut ed = Editor::new();
        let srv = srv_with(|_, _| {
            Some((
                0,
                vec![
                    Candidate {
                        display: "less".into(),
                        replacement: "less ".into(),
                    },
                    Candidate {
                        display: "ls".into(),
                        replacement: "ls ".into(),
                    },
                    Candidate {
                        display: "lsof".into(),
                        replacement: "lsof ".into(),
                    },
                ],
            ))
        });
        type_line(&mut ed, &srv, "l");
        // First Tab: longest common prefix = "l".
        ed.handle(Key::Tab, &srv);
        assert_eq!(ed.buffer(), "l");
        // Second Tab: show the list.
        ed.handle(Key::Tab, &srv);
        let r = ed.render(&srv);
        assert_eq!(r.completion.unwrap(), vec!["less", "ls", "lsof"]);
        // Third Tab: no-op.
        assert_eq!(ed.handle(Key::Tab, &srv), EditEvent::NoChange);
    }

    #[test]
    fn tab_lcp_extends_word() {
        let mut ed = Editor::new();
        let srv = srv_with(|_, _| {
            Some((
                0,
                vec![
                    Candidate {
                        display: "export".into(),
                        replacement: "export ".into(),
                    },
                    Candidate {
                        display: "exported".into(),
                        replacement: "exported ".into(),
                    },
                ],
            ))
        });
        type_line(&mut ed, &srv, "ex");
        ed.handle(Key::Tab, &srv);
        assert_eq!(ed.buffer(), "export");
        assert_eq!(ed.cursor_byte(), 6);
    }

    #[test]
    fn tab_no_matches_is_noop() {
        let mut ed = Editor::new();
        let srv = srv_with(|_, _| None);
        type_line(&mut ed, &srv, "zzz");
        assert_eq!(ed.handle(Key::Tab, &srv), EditEvent::NoChange);
        assert_eq!(ed.buffer(), "zzz");
    }

    #[test]
    fn editing_clears_completion() {
        let mut ed = Editor::new();
        let srv = srv_with(|_, _| {
            Some((
                0,
                vec![
                    Candidate {
                        display: "less".into(),
                        replacement: "less ".into(),
                    },
                    Candidate {
                        display: "ls".into(),
                        replacement: "ls ".into(),
                    },
                ],
            ))
        });
        type_line(&mut ed, &srv, "l");
        ed.handle(Key::Tab, &srv);
        assert!(ed.completion.is_some());
        ed.handle(Key::Char('x'), &srv);
        assert!(ed.completion.is_none());
    }

    #[test]
    fn ctrl_l_sets_flag() {
        let mut ed = Editor::new();
        let srv = srv();
        type_line(&mut ed, &srv, "hi");
        assert!(!ed.take_clear_screen());
        ed.handle(Key::Ctrl(CtrlKey::L), &srv);
        assert!(ed.take_clear_screen());
        assert!(!ed.take_clear_screen());
    }

    #[test]
    fn esc_cancels_completion() {
        let mut ed = Editor::new();
        let srv = srv_with(|_, _| {
            Some((
                0,
                vec![
                    Candidate {
                        display: "less".into(),
                        replacement: "less ".into(),
                    },
                    Candidate {
                        display: "ls".into(),
                        replacement: "ls ".into(),
                    },
                ],
            ))
        });
        type_line(&mut ed, &srv, "l");
        ed.handle(Key::Tab, &srv);
        ed.handle(Key::Esc, &srv);
        assert!(ed.completion.is_none());
    }
}
